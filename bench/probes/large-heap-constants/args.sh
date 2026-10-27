# The drift workload's arguments for each job of docs/research/large-heap-constants.md at each
# rung of live memory, in GiB. Sourced by ladder.sh and timed.sh.

# Fixed-point arithmetic on the rung: hundredths of a GiB.
centi() { awk -v g="$1" 'BEGIN { printf "%d", g * 100 + 0.5 }'; }

args_of() {
  local job="$1" g="$2" c
  c=$(centi "$g")
  case "$job" in
    # A shared cache of small values (median 128 B) at the rung, every value turned over
    # several times: 0.4G operations a GiB.
    steady) echo "shape=steady live=$((c * 1024 / 100))Mi ops=$((c * 4))M samples=60" ;;
    # Short-lived values whose total alternates between 0.79 and 0.066 of the rung, beside a
    # long-lived cache of 0.2 of it: peaks of about the rung, troughs of about a quarter. Six
    # phases of 1.2G operations, half a minute or more each, so that a trough lasts long
    # enough to see memory come back while the program runs; 26 samples a phase.
    lifetime) echo "shape=lifetime short_hi=$((c * 809 / 100))Mi short_lo=$((c * 68 / 100))Mi long=$((c * 205 / 100))Mi phases=6 ops=7200M samples=156" ;;
    # Large buffers, 40 KiB to 32 MiB by phase, at the rung: two cycles of the four ranges,
    # 20k operations a GiB a phase.
    large) echo "shape=large live=$((c * 1024 / 100))Mi phases=8 ops=$((c * 1600)) samples=64" ;;
    *) echo "unknown job $job" >&2; return 2 ;;
  esac
}
