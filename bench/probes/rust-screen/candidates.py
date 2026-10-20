"""The candidates screen.py runs: a command, the CPUs it may use, and what to do before each run.

Placeholders: {bin} is ROOT/bin, {inp} ROOT/inputs, {work} the candidate's working directory
(ROOT/work/NAME, the command's cwd), {root} ROOT. `setup` lines run in a shell in {work} before
every run, untimed. `cpus` is a taskset list: "2" for single-threaded programs, "0-3,8-11"
(one thread per physical core on both L3s, as the suite pins) for parallel ones; Rust's
available_parallelism follows it. A server entry gives `server_cmd` (measured, preloaded) and
`server.client` (not preloaded, pinned to `server.client_cpus`).
"""

MT = "0-3,8-11"
ST = "2"
SERVER_CPUS = "0-3"
CLIENT_CPUS = "8-11"

CANDIDATES = {
    # Tools and short-lived processes.
    "ripgrep": {
        # The tree four times over, so the run lasts a second; the walker hands paths across threads.
        "cmd": ["{bin}/ripgrep/rg", "--no-ignore", "-n", "-i", r"\w+_(alloc|free|lock|unlock)\w*\s*\("]
        + ["{inp}/linux-6.12"] * 4,
        "cpus": MT,
    },
    "tokei": {
        "cmd": ["{bin}/tokei/tokei", "--sort", "lines", "{inp}/linux-6.12", "{inp}/Python-3.13.0"],
        "cpus": MT,
    },
    "fd-loop": {
        # Short-lived processes: 30 runs of fd over the kernel tree in one shell.
        "cmd": ["sh", "-c", "for i in $(seq 30); do {bin}/fd/fd -u -e c . {inp}/linux-6.12; done | wc -l"],
        "cpus": MT,
    },
    "bat": {
        "setup": ["[ -s big.c ] || cat {inp}/linux-6.12/mm/*.c > big.c"],
        "cmd": ["{bin}/bat/bat", "--color=always", "--paging=never", "--style=numbers", "-l", "c", "big.c"],
        "cpus": ST,
    },
    "difftastic": {
        "cmd": ["{bin}/difftastic/difft", "--color=always", "--display=inline",
                "{inp}/Python-3.12.7/Lib/asyncio", "{inp}/Python-3.13.0/Lib/asyncio"],
        "cpus": MT,
        "ok_rc": [0, 1],
    },
    "pulldown-cmark": {
        "cmd": ["{bin}/pulldown-cmark/pulldown-cmark", "-T", "-F", "-S"] + ["{inp}/md/corpus.md"] * 4,
        "cpus": ST,
    },
    "cargo-resolve": {
        # Cargo's resolver, offline, over ruff's workspace (about 600 packages) ten times. It
        # resolves against the local registry index, which other builds on the machine extend, so
        # the result is not pinned.
        "setup": ["[ -d ws ] || cp -r {root}/src/ruff ws"],
        "cmd": ["sh", "-c", "for i in $(seq 10); do rm -f ws/Cargo.lock; {root}/toolchain-cargo generate-lockfile "
                "--offline -q --manifest-path ws/Cargo.toml; done; wc -l < ws/Cargo.lock"],
        "env": {"CARGO_NET_OFFLINE": "true"},
        "cpus": ST,
    },
    "uv-loop": {
        # Short-lived processes: 20 offline resolutions of a 397-package set from a cache filled
        # once online (inputs/uv; --exclude-newer pins the index's state).
        "cmd": ["sh", "-c", "for i in $(seq 20); do {bin}/uv/uv pip compile --offline --python-version 3.12 "
                "--exclude-newer 2026-10-01 --python-platform x86_64-unknown-linux-gnu -q {inp}/uv/requirements.in; "
                "done | wc -l"],
        "env": {"UV_CACHE_DIR": "{inp}/uv/cache"},
        "cpus": MT,
    },
    "gix-hours": {
        # gitoxide's history walk with a tree diff and line diff of every commit: ein's
        # estimate-hours over rust-lang/cargo up to tag 0.90.0 (c24e1064), 20k commits.
        "cmd": ["{bin}/gix/ein", "tool", "estimate-hours", "--file-stats", "--line-stats", "{inp}/repo/cargo.git",
                "c24e1064277fe51ab72011e2612e556ac56addf7"],
        "cpus": MT,
    },
    # Compilers and analyzers.
    "ruff-check": {
        "cmd": ["{bin}/ruff/ruff", "check", "--no-cache", "--select", "ALL", "--exit-zero", "-q",
                "--output-format", "concise", "{inp}/Python-3.13.0/Lib"],
        "cpus": MT,
    },
    "ruff-check-st": {
        # The same on one thread, to tell the allocator's own cost from how the parallel run
        # schedules.
        "cmd": ["{bin}/ruff/ruff", "check", "--no-cache", "--select", "ALL", "--exit-zero", "-q",
                "--output-format", "concise", "{inp}/Python-3.13.0/Lib"],
        "env": {"RAYON_NUM_THREADS": "1"},
        "cpus": ST,
    },
    "ruff-format": {
        "cmd": ["{bin}/ruff/ruff", "format", "--no-cache", "--diff", "{inp}/Python-3.13.0/Lib"],
        "cpus": MT,
        "ok_rc": [0, 1, 2],
    },
    "ty": {
        "cmd": ["{bin}/ruff/ty", "check", "--exit-zero", "--python-version", "3.13", "--output-format", "concise",
                "{inp}/Python-3.13.0/Lib"],
        "cpus": MT,
        "ok_rc": [0, 1],
    },
    "rust-analyzer": {
        # ripgrep 15.2.0's crates.io source (with an empty [workspace] appended, since it sits under
        # this checkout's) and its dependencies from the registry, with std from rust-src 1.99.1.
        "cmd": ["{bin}/rust-analyzer/rust-analyzer", "analysis-stats", "--disable-build-scripts",
                "--disable-proc-macros", "{inp}/ra/ripgrep-15.2.0"],
        "env": {"RUST_SRC_PATH": "{inp}/rust-src-1.99.1/rust-src/lib/rustlib/src/rust/library",
                "CARGO_NET_OFFLINE": "true", "CARGO": "{root}/toolchain-cargo"},
        "cpus": MT,
    },
    "oxlint": {
        # Files named one by one: oxlint's walker honours this checkout's .gitignore, which
        # ignores every input under target/.
        "cmd": ["sh", "-c", "{bin}/oxc/oxlint -W all --silent $(find {inp}/js -name '*.js' | sort)"],
        "cpus": MT,
        "ok_rc": [0, 1],
    },
    "typst": {
        "cmd": ["{bin}/typst/typst", "compile", "{inp}/typst/doc.typ", "{work}/out.pdf"],
        "cpus": MT,
    },
    "wasmtime": {
        "cmd": ["{bin}/wasmtime/wasmtime", "compile", "-o", "{work}/out.cwasm",
                "{inp}/js/esbuild-wasm-0.24.0/package/esbuild.wasm"],
        "cpus": MT,
    },
    "egglog": {
        # Four of egglog's larger tests, one process each (they cannot share an e-graph); they
        # include files relative to the crate.
        "setup": ["ln -sfn {root}/src/egglog/tests tests"],
        "cmd": ["sh", "-c", "for t in eggcc-2mm taylor51 math-microbenchmark stresstest_large_expr; do "
                "{bin}/egglog/egglog tests/$t.egg || exit 1; done"],
        "cpus": ST,
    },
    # Data.
    "jaq": {
        "cmd": ["{bin}/jaq/jaq", "-c",
                ".features | map(select(.properties.BLOCK_NUM != null)) | group_by(.properties.STREET)"
                " | map({{s: .[0].properties.STREET, n: length, lots: map(.properties.LOT_NUM)}}) | length",
                "{inp}/dl/citylots.json"],
        "cpus": ST,
    },
    "tantivy": {
        "setup": ["rm -rf idx && mkdir idx && cp {inp}/tantivy/meta.json idx/"],
        "cmd": ["{bin}/tantivy/tantivy", "index", "-i", "idx", "-f", "{inp}/tantivy/docs.jsonl", "-t", "8"],
        "cpus": MT,
    },
    "tpchgen": {
        "setup": ["rm -rf out"],
        "cmd": ["{bin}/tpchgen/tpchgen-cli", "parquet", "-s", "1", "--output-dir=out"],
        "cpus": MT,
    },
    "datafusion": {
        "cmd": ["{bin}/datafusion/datafusion-cli", "-q", "-f", "{inp}/tpch/run.sql"],
        "cpus": MT,
    },
    "xsv": {
        "cmd": ["{bin}/xsv/xsv", "frequency", "-s", "2,3,9,10,15", "{inp}/tpch/csv/lineitem.csv"],
        "cpus": ST,
    },
    "parquet-rewrite": {
        "cmd": ["{bin}/parquet/parquet-rewrite", "-i", "{inp}/tpch/sf1/lineitem.parquet", "-o", "{work}/out.parquet",
                "--compression", "zstd"],
        "cpus": ST,
    },
    # Interpreters.
    "boa": {
        # Octane's splay (a tree of objects churned through the collector), a quarter of its
        # deterministic iterations; the other Octane programs run for minutes in boa.
        "cmd": ["{bin}/boa/boa", "{inp}/octane-run/splay.js"],
        "cpus": ST,
    },
    "rustpython": {
        "cmd": ["{bin}/rustpython/rustpython", "{inp}/py/bench.py"],
        "cpus": ST,
    },
    "rhai": {
        "cmd": ["{bin}/rhai/rhai-run", "{inp}/rhai/bench.rhai"],
        "cpus": ST,
    },
    "starlark": {
        "cmd": ["{bin}/starlark/starlark", "{inp}/starlark/bench.star"],
        "cpus": ST,
    },
    "wasmi": {
        # pulldown-cmark built for wasm32-wasip1, interpreted: the host allocates little once
        # the module is translated, so this is expected to be a control.
        "cmd": ["{bin}/wasmi/wasmi", "--dir", "{inp}/md", "{inp}/wasm/pulldown-cmark.wasm", "{inp}/md/corpus.md",
                "{inp}/md/corpus.md"],
        "cpus": ST,
    },
    # Parallel compute.
    "rav1e": {
        "cmd": ["{bin}/rav1e/rav1e", "{inp}/dl/akiyo_cif.y4m", "-o", "{work}/out.ivf", "--speed", "6", "--limit", "60",
                "--threads", "8", "-y"],
        "cpus": MT,
    },
    "oxipng": {
        "setup": ["rm -rf out && mkdir out"],
        "cmd": ["sh", "-c", "{bin}/oxipng/oxipng -o 4 --dir out {inp}/png/*.png"],
        "cpus": MT,
    },
    # Servers: measured process on four cores of one L3, the closed-loop client on the other.
    "axum": {
        "server_cmd": ["{bin}/servers/axum-orders", "18080"],
        "server": {"port": 18080, "client_cpus": CLIENT_CPUS,
                   "client": ["{bin}/oha/oha", "--no-tui", "-n", "400000", "-c", "64", "-m", "POST", "-T",
                              "application/json", "-D", "{root}/../../bench/probes/rust-screen/servers/order.json",
                              "http://127.0.0.1:18080/orders"]},
        "cpus": SERVER_CPUS,
        "spin_cpus": MT,
    },
    "actix": {
        "server_cmd": ["{bin}/servers/actix-orders", "18081"],
        "server": {"port": 18081, "client_cpus": CLIENT_CPUS,
                   "client": ["{bin}/oha/oha", "--no-tui", "-n", "400000", "-c", "64", "-m", "POST", "-T",
                              "application/json", "-D", "{root}/../../bench/probes/rust-screen/servers/order.json",
                              "http://127.0.0.1:18081/orders"]},
        "cpus": SERVER_CPUS,
        "spin_cpus": MT,
    },
    "mini-redis": {
        "server_cmd": ["{bin}/mini-redis/mini-redis-server", "--port", "16379"],
        "server": {"port": 16379, "client_cpus": CLIENT_CPUS,
                   # Not pipelined: mini-redis writes each reply on its own without TCP_NODELAY, so a
                   # pipelined batch waits on Nagle and the client's delayed ACK, 40 ms a batch.
                   "client": ["{bin}/servers/resp-load", "127.0.0.1:16379", "64", "1", "1600000"]},
        "cpus": SERVER_CPUS,
        "spin_cpus": MT,
    },
}

# What each candidate runs, in a few words, for the research note's table.
INPUTS = {
    "ripgrep": "regex over Linux 6.12 ×4",
    "tokei": "Linux 6.12 + CPython 3.13 trees",
    "fd-loop": "30 walks of Linux 6.12",
    "bat": "highlight mm/*.c, 6 MB",
    "difftastic": "asyncio, CPython 3.12.7 vs 3.13.0",
    "pulldown-cmark": "470 MB of reST read as Markdown",
    "cargo-resolve": "resolve ruff's workspace ×10",
    "uv-loop": "resolve 397 packages offline ×20",
    "gix-hours": "estimate-hours, cargo repo, 20k commits",
    "ruff-check": "check --select ALL, CPython Lib",
    "ruff-check-st": "same, one thread",
    "ruff-format": "format --diff, CPython Lib",
    "ty": "check CPython Lib",
    "rust-analyzer": "analysis-stats on ripgrep",
    "oxlint": "-W all, 2805 npm .js files",
    "typst": "generated 480-section report",
    "wasmtime": "compile esbuild.wasm, 12 MB",
    "egglog": "four of its larger tests",
    "jaq": "group_by over citylots.json, 189 MB",
    "tantivy": "index 300 MB of docs and C source",
    "tpchgen": "TPC-H SF1 to Parquet",
    "datafusion": "TPC-H SF1, 22 queries",
    "xsv": "frequency, lineitem SF1 CSV",
    "parquet-rewrite": "lineitem SF1 to zstd",
    "boa": "Octane splay, 1/4 iterations",
    "rustpython": "richards, strings, trees, dict churn",
    "rhai": "maps, arrays, closures script",
    "starlark": "BUILD-like dicts and lists",
    "wasmi": "pulldown-cmark.wasm over 240 MB",
    "rav1e": "60 CIF frames, speed 6",
    "oxipng": "-o 4 on 40 PNGs",
    "axum": "POST JSON orders, 400k, 64 conns",
    "actix": "POST JSON orders, 400k, 64 conns",
    "mini-redis": "1.6M GET/SET, 64 conns",
}
