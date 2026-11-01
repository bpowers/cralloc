#!/usr/bin/env bash
# Build the C-entry study into out/ (gitignored): the routines as a shared library and linked
# into the driver, and the driver twice, with a plain PLT and with the IBT PLT (.plt.sec) that
# distributions' hardened builds produce. Builds stay on BUILD_CPUS (default 4-7,20-23).
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
out=$here/out
cpus=${BUILD_CPUS:-4-7,20-23}
run() { taskset -c "$cpus" "$@"; }
mkdir -p "$out"
run python3 "$here/gen.py" "$out"
# No relaxation: a GOT load stays a GOT load, so both copies run the instructions written.
as_flags=-Wa,-mrelax-relocations=no
run cc -c -fPIC $as_flags -o "$out/routines_lib.o" "$out/routines_lib.S"
run cc -shared -Wl,-z,now -o "$out/libcentry.so" "$out/routines_lib.o"
run cc -c -fPIE $as_flags -o "$out/routines_exe.o" "$out/routines_exe.S"
run cc -c -fPIE $as_flags -o "$out/kernels.o" "$out/kernels.S"
run cc -O2 -fPIE -Wall -Wextra -I "$out" -I "$here" -c -o "$out/bench.o" "$here/bench.c"
objs=("$out/bench.o" "$out/routines_exe.o" "$out/kernels.o")
link=(-pie -Wl,-z,now -L "$out" -lcentry -Wl,-rpath,"$out")
run cc -o "$out/centry" "${objs[@]}" "${link[@]}"
run cc -o "$out/centry-ibt" "${objs[@]}" "${link[@]}" -Wl,-z,ibtplt
# The stand-in allocators (standin.c) around two shapes, and the C loops in the three ways C
# code calls a shared library: the PLT, the IBT PLT, and -fno-plt's call through the GOT.
sflags=(-O2 -fPIC -shared -Wall -Wextra -I "$out" -I "$here" -fno-builtin -Wl,-z,now)
for tag in best bestnc two rust; do
  run cc -c -fPIC $as_flags -o "$out/standin_$tag.o" "$out/standin_$tag.S"
  calloc=1
  case $tag in best) calloc=3 ;; esac
  run cc "${sflags[@]}" -DCALLOC=$calloc -o "$out/libstandin-$tag.so" "$here/standin.c" "$out/standin_$tag.o"
done
# calloc as C over the best malloc: through the PLT (0), a direct call with 16-byte stores (1),
# and a direct call with `rep stosb` (2).
for k in 0 1 2; do
  run cc "${sflags[@]}" -DCALLOC=$k -o "$out/libstandin-best-c$k.so" "$here/standin.c" "$out/standin_bestnc.o"
done
cflags=(-O2 -fPIE -pie -Wall -Wextra -Wl,-z,now)
run cc "${cflags[@]}" -o "$out/cloop-plt" "$here/cloop.c"
run cc "${cflags[@]}" -fcf-protection=full -Wl,-z,ibtplt -o "$out/cloop-ibt" "$here/cloop.c"
run cc "${cflags[@]}" -fno-plt -o "$out/cloop-noplt" "$here/cloop.c"
run c++ "${cflags[@]}" -o "$out/cxxloop-plt" "$here/cxxloop.cpp"
run c++ "${cflags[@]}" -fno-plt -o "$out/cxxloop-noplt" "$here/cxxloop.cpp"
# The loops linked with cralloc itself as a static library (static/), so that they call its
# generated routines directly, as a statically linked program does.
(cd "$here/static" && run cargo build --release -q)
run cc "${cflags[@]}" -o "$out/cloop-static" "$here/cloop.c" "$here/static/target/release/libcentry_static.a"
echo "built $out"
