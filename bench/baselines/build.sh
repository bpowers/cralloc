#!/usr/bin/env bash
# Rebuild every baseline allocator, and the C benchmark programs, from scratch.
#
#   bench/baselines/build.sh [component ...]
#
# Components: mimalloc tcmalloc jemalloc cbench probe (default: all). Output goes to
# bench/baselines/out of the main checkout, so that every worktree shares one set and removing
# a worktree loses nothing; BENCH_BASELINES overrides it. It is gitignored. A component's
# directory is deleted before it is rebuilt. Sources are read from third_party/ in the main
# checkout and never written: tcmalloc is copied out before bazel sees it, because bazel
# writes a lock file and symlinks into the tree it builds.
#
# A build holds out/.lock exclusively, and a benchmark session holds it shared, so the
# libraries cannot change under a running session.
#
# Builds are confined to BENCH_BUILD_CPUS (default the set `heavy` of scripts/cpusets) so they
# stay off the CPUs that measurements use. Start it through scripts/heavy. On aarch64 everything is
# built for the host's CPU and page size, as `.cargo/config.toml` builds the Rust: inline LSE atomics
# for all, since an allocator left on outlined atomics would be handicapped by our build and not
# by its design. out/manifest/ records versions, commits and flags.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
# The main checkout: the parent of the git directory every worktree shares.
main=$(dirname "$(git -C "$here" rev-parse --path-format=absolute --git-common-dir)")
out=${BENCH_BASELINES:-$main/bench/baselines/out}
cpus=${BENCH_BUILD_CPUS:-$("$here/../../scripts/cpusets" heavy)}
jobs=$(taskset -c "$cpus" nproc)
bazel=${BAZEL:-$HOME/bin/bazel}
tp=${THIRD_PARTY:-$main/third_party}
[ -d "$tp/mimalloc" ] || { echo "no third_party checkouts at $tp (set THIRD_PARTY)" >&2; exit 1; }
case "$(uname -m)" in
  aarch64) march=(-mcpu=native) ;;
  *) march=() ;;
esac

components=("$@")
[ ${#components[@]} -gt 0 ] || components=(probe jemalloc mimalloc cbench tcmalloc)

run() { taskset -c "$cpus" "$@"; }
mkdir -p "$out/manifest"
exec 9>"$out/.lock"
flock -n 9 || { echo "$out is in use by another build or a benchmark session" >&2; exit 1; }
manifest() { cat > "$out/manifest/$1.txt"; }
head_of() { git -C "$tp/$1" rev-parse HEAD; }
describe_of() { git -C "$tp/$1" describe --tags --always 2>/dev/null || true; }
# --no-optional-locks keeps status from refreshing the checkout's index, which is a write.
dirty_of() {
  [ -z "$(git --no-optional-locks -C "$tp/$1" status --porcelain --untracked-files=no)" ] && echo clean || echo dirty
}

host_manifest() {
  manifest host <<EOF
date: $(date -u +%Y-%m-%dT%H:%M:%SZ)
kernel: $(uname -r)
glibc: $(ldd --version | head -1)
cc: $(cc --version | head -1)
c++: $(c++ --version | head -1)
cmake: $(cmake --version | head -1)
rustc: $(rustc --version)
page size: $(getconf PAGESIZE)
compiler's reading of the CPU flags: ${march[*]:-none} $(cc "${march[@]}" -Q --help=target 2>/dev/null | sed -n 's/^ *-mcpu= *//p')
EOF
}

build_probe() {
  # A preloaded library that reports at exit which allocator served the process and,
  # for tcmalloc, whether its per-CPU caches were active (bench/baselines/probe.c).
  rm -rf "$out/probe"; mkdir -p "$out/probe"
  local flags="-O2 -fPIC -shared -Wall -Wextra"
  run cc $flags -o "$out/probe/libbenchprobe.so" "$here/probe.c" -ldl
  manifest probe <<EOF
source: bench/baselines/probe.c
flags: cc $flags
EOF
}

build_jemalloc() {
  # On aarch64, from source: jemalloc takes the page size at configure time, so a distribution's
  # package is for the kernels it chose, and its atomics are outlined.
  if [ ${#march[@]} -gt 0 ]; then build_jemalloc_source; return; fi
  # The distribution package, for LD_PRELOAD runs: what a C program gets from jemalloc on
  # this system. Nothing to build; record what it is. Native Rust runs use the static build
  # that tikv-jemallocator compiles (bench/alloc), which cargo builds and records.
  local lib=/usr/lib64/libjemalloc.so.2
  [ -e "$lib" ] || { echo "missing $lib (install jemalloc-devel)" >&2; exit 1; }
  rm -rf "$out/jemalloc"; mkdir -p "$out/jemalloc"
  ln -s "$lib" "$out/jemalloc/libjemalloc.so"
  {
    echo "package: $(rpm -q jemalloc 2>/dev/null || echo unknown)"
    echo "library: $lib"
    echo "--- stats_print at exit of /bin/true:"
    LD_PRELOAD=$lib MALLOC_CONF=stats_print:true /bin/true 2>&1 | sed -n '/^Version/,/^Run-time/p'
  } | manifest jemalloc
}

build_jemalloc_source() {
  # The version the distribution ships, with its default configuration otherwise.
  rm -rf "$out/jemalloc"; mkdir -p "$out/jemalloc"
  tar -C "$tp" --exclude=.git -cf - jemalloc | tar -C "$out/jemalloc" -xf -
  local src=$out/jemalloc/jemalloc lg_page version
  lg_page=$(python3 -I -c 'import os; print(os.sysconf("SC_PAGESIZE").bit_length() - 1)')
  version=$(git --no-optional-locks -C "$tp/jemalloc" describe --long --always)
  local configure=(--with-lg-page="$lg_page" --with-version="$version" --enable-doc=no)
  # 5.3.0 calls an internal of libstdc++ that GCC 16 no longer declares through <new>.
  sed -i 's/std::__throw_bad_alloc()/throw std::bad_alloc()/' "$src/src/jemalloc_cpp.cpp"
  (cd "$src" && autoconf && CFLAGS="${march[*]}" CXXFLAGS="${march[*]}" run ./configure "${configure[@]}") \
    > "$out/jemalloc/configure.log"
  run make -C "$src" -j "$jobs" build_lib_shared > "$out/jemalloc/build.log"
  cp -L "$src/lib/libjemalloc.so.2" "$out/jemalloc/libjemalloc.so"
  {
    echo "source: third_party/jemalloc $(describe_of jemalloc) $(head_of jemalloc) ($(dirty_of jemalloc))"
    echo "configure: CFLAGS=${march[*]} CXXFLAGS=${march[*]} ./configure ${configure[*]}"
    echo "patched: jemalloc_cpp.cpp throws std::bad_alloc itself, for GCC 16"
    echo "--- stats_print at exit of /bin/true:"
    LD_PRELOAD=$out/jemalloc/libjemalloc.so MALLOC_CONF=stats_print:true /bin/true 2>&1 |
      sed -n '/^Version/,/^Run-time/p;/Page size/p'
  } | manifest jemalloc
}

build_mimalloc() {
  # Two builds of the same source. For Rust, a static archive compiled the way the
  # `libmimalloc-sys` crate compiles it: the single translation unit src/static.c, its
  # defines, no override, so Rust
  # calls mi_* by name and C malloc stays glibc's. For LD_PRELOAD, the shared library with
  # override that mimalloc's own cmake build produces.
  rm -rf "$out/mimalloc"; mkdir -p "$out/mimalloc"
  local static=(-O3 "${march[@]}" -fPIC -ffunction-sections -fdata-sections -std=gnu11 -DMI_DEBUG=0 -DMI_BUILD_RELEASE
                -DNDEBUG -ftls-model=initial-exec -Wno-error=date-time
                -I "$tp/mimalloc/include" -I "$tp/mimalloc/src")
  run cc "${static[@]}" -c "$tp/mimalloc/src/static.c" -o "$out/mimalloc/static.o"
  ar rcs "$out/mimalloc/libmimalloc.a" "$out/mimalloc/static.o"
  local shared=(-DCMAKE_BUILD_TYPE=Release -DMI_BUILD_TESTS=OFF -DMI_BUILD_OBJECT=OFF
                -DMI_OVERRIDE=ON -DMI_BUILD_SHARED=ON -DMI_BUILD_STATIC=OFF "-DCMAKE_C_FLAGS=${march[*]}")
  run cmake -S "$tp/mimalloc" -B "$out/mimalloc/build-shared" "${shared[@]}" > "$out/mimalloc/cmake-shared.log"
  run cmake --build "$out/mimalloc/build-shared" -j "$jobs" -- VERBOSE=1 > "$out/mimalloc/build-shared.log"
  cp -L "$out/mimalloc/build-shared/libmimalloc.so" "$out/mimalloc/"
  manifest mimalloc <<EOF
source: third_party/mimalloc $(describe_of mimalloc) $(head_of mimalloc) ($(dirty_of mimalloc))
version: $(sed -n 's/#define MI_MALLOC_VERSION *\([0-9]*\).*/\1/p' "$tp/mimalloc/include/mimalloc.h")
static (Rust #[global_allocator]), as libmimalloc-sys 0.1.49 builds it: cc ${static[*]} -c src/static.c
shared (LD_PRELOAD): cmake ${shared[*]}
compile line (shared): $(grep -m1 ' -c .*alloc\.c' "$out/mimalloc/build-shared.log")
EOF
}

build_tcmalloc() {
  rm -rf "$out/tcmalloc"; mkdir -p "$out/tcmalloc/src"
  # Copy the tree without its git metadata, then add our two targets next to it.
  tar -C "$tp" --exclude=.git --exclude="tcmalloc/bazel-*" -cf - tcmalloc | tar -C "$out/tcmalloc/src" -xf -
  local ws=$out/tcmalloc/src/tcmalloc
  mkdir -p "$ws/bench_shim"
  cp "$here/tcmalloc-shim/BUILD.bazel" "$ws/bench_shim/BUILD.bazel"
  # The registry's abseil-cpp changed compatibility level between the release protobuf asks for
  # and the one tcmalloc asks for, which bazel refuses unless the root chooses one: tcmalloc's.
  local absl
  absl=$(sed -n 's/^bazel_dep(name = "abseil-cpp", version = "\([^"]*\)".*/\1/p' "$ws/MODULE.bazel")
  echo "single_version_override(module_name = \"abseil-cpp\", version = \"$absl\")" >> "$ws/MODULE.bazel"
  local root=$out/tcmalloc/bazel-root
  # Initial-exec TLS, as preloaded allocators are built (mimalloc's and jemalloc's shared
  # builds use it for their thread-locals). Without it, -fPIC with this GCC's default
  # -mtls-dialect=gnu2 reaches every thread-local through a TLS descriptor call, an indirect
  # call through memory that on Zen 5 defeats zero-latency store forwarding: 41 to 57 cycles
  # per malloc and free pair (docs/research/roofline.md, "A reload that loses memory renaming
  # costs 5 to 7 cycles"). Static links relax either model to local-exec, so this changes only
  # the shared library.
  local flags=(-c opt --symlink_prefix=/ --jobs="$jobs" --local_resources=cpu="$jobs"
               --experimental_cc_static_library --copt=-ftls-model=initial-exec "${march[@]/#/--copt=}")
  # The server outlives this script, so it must not inherit the lock, and must not linger either.
  local startup=(--output_user_root="$root" --max_idle_secs=60)
  (cd "$ws" && run "$bazel" "${startup[@]}" build "${flags[@]}" \
      //bench_shim:libtcmalloc.so //bench_shim:tcmalloc_static) 9>&- 2>&1 | tee "$out/tcmalloc/build.log" | tail -5
  local bin
  bin=$(cd "$ws" && "$bazel" "${startup[@]}" info "${flags[@]}" bazel-bin 9>&- 2>/dev/null)
  cp "$bin/bench_shim/libtcmalloc.so" "$out/tcmalloc/libtcmalloc.so"
  cp "$bin/bench_shim/libtcmalloc_static.a" "$out/tcmalloc/libtcmalloc.a"
  chmod u+w "$out/tcmalloc/"lib*
  (cd "$ws" && "$bazel" "${startup[@]}" shutdown 9>&-) || true
  # Bazel autodetects the C++ toolchain; record what it found and what -c opt means there.
  local cc_build cxx opt
  cc_build=$(find "$root" -path '*external/*local_config_cc/BUILD' | head -1)
  cxx=$(sed -n 's/.*"gcc": "\([^"]*\)".*/\1/p' "$cc_build" | head -1)
  opt=$(sed -n '/opt_compile_flags = \[/,/\]/p' "$cc_build" | tr -d '\n ' | sed 's/opt_compile_flags=//')
  cp "$ws/MODULE.bazel.lock" "$out/manifest/tcmalloc-MODULE.bazel.lock"
  manifest tcmalloc <<EOF
source: third_party/tcmalloc $(head_of tcmalloc) ($(dirty_of tcmalloc))
bazel: $("$bazel" --version)
flags: bazel build ${flags[*]} (plus the repository's .bazelrc: --cxxopt=-std=c++17)
compiler: $cxx ($($cxx --version | head -1)); -c opt adds $opt
targets: bench/baselines/tcmalloc-shim/BUILD.bazel (cc_binary linkshared and cc_static_library over //tcmalloc, 8 KiB pages)
module deps: MODULE.bazel; resolved versions in tcmalloc-MODULE.bazel.lock
mode: per-CPU caches only with GLIBC_TUNABLES=glibc.pthread.rseq=0; per-thread otherwise
EOF
}

build_cbench() {
  # Selected mimalloc-bench programs, compiled with the flags of its CMakeLists.txt
  # (Release: -O3 -DNDEBUG, warnings off). sh8bench is not redistributable; it is
  # downloaded, checked against mimalloc-bench's checksum, and patched as its script does.
  local src=$tp/mimalloc-bench/bench
  local o=$out/cbench
  rm -rf "$o"; mkdir -p "$o/dl"
  local cf="-O3 ${march[*]} -DNDEBUG -w -Wno-implicit-function-declaration -Wno-implicit-int -Wno-int-conversion -Wno-return-mismatch -Wno-incompatible-pointer-types"
  local cxf="-O3 ${march[*]} -DNDEBUG -std=c++17 -w"
  run c++ $cxf -DCPP=1 -o "$o/larson" "$src/larson/larson.cpp" -lpthread
  run c++ $cxf -fsized-deallocation -DCPP=1 -DSIZED=1 -o "$o/larson-sized" "$src/larson/larson.cpp" -lpthread
  run cc $cf -o "$o/xmalloc-test" "$src/xmalloc-test/xmalloc-test.c" -lpthread
  run cc $cf -o "$o/mstress" "$src/mstress/mstress.c" -lpthread
  run c++ $cxf -o "$o/cache-scratch" "$src/cache-scratch/cache-scratch.cpp" -lpthread
  local cfrac=(cfrac.c pops.c pconst.c pio.c pabs.c pneg.c pcmp.c podd.c phalf.c padd.c psub.c
               pmul.c pdivmod.c psqrt.c ppowmod.c atop.c ptoa.c itop.c utop.c ptou.c errorp.c
               pfloat.c pidiv.c pimod.c picmp.c primes.c pcfrac.c pgcd.c)
  run cc $cf -std=gnu89 -DNOMEMOPT=1 -o "$o/cfrac" "${cfrac[@]/#/$src/cfrac/}" -lm
  local esp=(cofactor.c cols.c compl.c contain.c cubestr.c cvrin.c cvrm.c cvrmisc.c cvrout.c
             dominate.c equiv.c espresso.c essen.c exact.c expand.c gasp.c getopt.c gimpel.c
             globals.c hack.c indep.c irred.c main.c map.c matrix.c mincov.c opo.c pair.c
             part.c primes.c reduce.c rows.c set.c setc.c sharp.c sminterf.c solution.c
             sparse.c unate.c utility.c verify.c)
  run cc $cf -std=gnu89 -o "$o/espresso" "${esp[@]/#/$src/espresso/}" -lm
  cp "$src/espresso/largest.espresso" "$o/"

  local sha=12a8e75248c9dcbfee28245c12bc937a16ef56ec9cbfab88d0e348271667726f
  curl -sSfL -o "$o/dl/SH8BENCH.zip" http://www.microquill.com/smartheap/SH8BENCH.zip
  echo "$sha  $o/dl/SH8BENCH.zip" | sha256sum -c --quiet
  (cd "$o/dl" && unzip -q SH8BENCH.zip && cp "$src/shbench/sh8bench.patch" . &&
     dos2unix -q SH8BENCH.C sh8bench.patch && patch -s -p1 -o sh8bench-new.c SH8BENCH.C sh8bench.patch)
  run cc $cf -DBENCH=1 -DSYS_MULTI_THREAD=1 -o "$o/sh8bench" "$o/dl/sh8bench-new.c" -lpthread
  manifest cbench <<EOF
source: third_party/mimalloc-bench $(head_of mimalloc-bench) ($(dirty_of mimalloc-bench))
sh8bench: http://www.microquill.com/smartheap/SH8BENCH.zip sha256 $sha, patched with bench/shbench/sh8bench.patch
c flags: cc $cf ; c++ flags: c++ $cxf
larson-sized: -fsized-deallocation -DCPP=1 -DSIZED=1 (operator delete with size)
cfrac, espresso: -std=gnu89; cfrac -DNOMEMOPT=1
EOF
}

host_manifest
for c in "${components[@]}"; do
  echo "== $c" >&2
  "build_$c"
done
echo "baselines in $out" >&2
