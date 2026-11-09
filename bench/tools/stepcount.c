// Exact instruction counts for one thread of a running program, by single-stepping it, rseq
// sequences included (docs/research/measuring.md, "Exact instruction counts on aarch64").
//
// aarch64 only: it finds the thread's rseq area through NT_ARM_TLS.
//
// Build:
//   cc -O2 -o /tmp/stepcount bench/tools/stepcount.c
// Run it through scripts/memlimit, since the program it starts is the allocator, and through
// scripts/heavy, since it needs no quiet machine:
//   BENCH_CPUS=4,5 scripts/heavy scripts/memlimit -m 4G --
//     /tmp/stepcount OUT THREAD STEPS WARM_MS -- PROGRAM [ARG...]
// Read the result with bench/tools/stepsum.py OUT PROGRAM.
//
// It starts PROGRAM, a path to a dynamically linked, position-independent executable using glibc's
// rseq area, lets it run free for WARM_MS, single-steps its THREADth thread (by ascending thread
// ID, 0 being the main thread) for STEPS instructions while the others run free, writes OUT and
// kills the program. Give it work to outlast the steps. OUT has a line `ADDRESS COUNT` for each
// instruction that ran: the address in hex, as `nm PROGRAM` gives it. Address 0 stands for
// everything outside PROGRAM.
//
// A ptrace stop counts as a preemption, which would abort a sequence at its next instruction, so
// stepping alone never gets through one. So at each stop this clears the thread's rseq_cs word,
// which is what the kernel reads to decide. The thread then has no protection, which is sound only
// if nothing a sequence guards can change under it. It must be pinned to one CPU, which is checked
// here, and alone there, which is not; and it must reach a slow path often enough that no other
// thread's idle check drains its slab. The measuring note says how often.
#define _GNU_SOURCE
#include <dirent.h>
#include <dlfcn.h>
#include <elf.h>
#include <errno.h>
#include <inttypes.h>
#include <sched.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ptrace.h>
#include <sys/rseq.h>
#include <sys/uio.h>
#include <sys/user.h>
#include <sys/wait.h>
#include <unistd.h>

#if !defined(__aarch64__)
#error "stepcount is aarch64 only: it reads the thread pointer with NT_ARM_TLS"
#endif

enum { MAX_THREADS = 4096 };

static pid_t program;

static void die(const char *message) {
  fprintf(stderr, "stepcount: %s\n", message);
  if (program > 0) kill(program, SIGKILL);
  exit(1);
}

static void usage(void) {
  fputs("usage: scripts/memlimit -m 4G -- stepcount OUT THREAD STEPS WARM_MS -- PROGRAM [ARG...]\n"
        "bench/tools/stepcount.c says what they mean\n",
        stderr);
  exit(2);
}

static uint64_t number(const char *text) {
  char *end;
  errno = 0;
  uint64_t value = strtoull(text, &end, 10);
  if (errno || end == text || *end) usage();
  return value;
}

// The span of the mappings whose lines in `maps` (of `pid`, 0 for this process) name `name`.
static int span_of(pid_t pid, const char *name, uint64_t *start, uint64_t *end) {
  char path[64], line[4096];
  if (pid)
    snprintf(path, sizeof path, "/proc/%d/maps", pid);
  else
    snprintf(path, sizeof path, "/proc/self/maps");
  FILE *f = fopen(path, "r");
  if (!f) return 0;
  int found = 0;
  while (fgets(line, sizeof line, f)) {
    uint64_t from, to;
    if (strstr(line, name) && sscanf(line, "%" SCNx64 "-%" SCNx64, &from, &to) == 2) {
      if (!found) *start = from;
      *end = to;
      found = 1;
    }
  }
  fclose(f);
  return found;
}

static int ascending(const void *a, const void *b) { return *(const int *)a - *(const int *)b; }

// The thread IDs of the program, ascending; how many.
static int threads_of(pid_t pid, int *tids) {
  char path[64];
  snprintf(path, sizeof path, "/proc/%d/task", pid);
  DIR *d = opendir(path);
  if (!d) die("the program has gone");
  int n = 0;
  struct dirent *e;
  while ((e = readdir(d)))
    if (e->d_name[0] != '.' && n < MAX_THREADS) tids[n++] = atoi(e->d_name);
  closedir(d);
  qsort(tids, n, sizeof(int), ascending);
  return n;
}

// Where the rseq_cs word is of the thread, which runs on CPU `cpu`. __rseq_offset is in the dynamic
// loader, the same file in this process and in the program, so it lies at the same distance from
// the loader's start. The area must name the CPU, or clearing a word of it would corrupt memory.
static uint64_t rseq_cs_of(pid_t tid, int cpu) {
  uint64_t own = 0, its = 0, end = 0;
  uint64_t symbol = (uint64_t)dlsym(RTLD_DEFAULT, "__rseq_offset");
  if (!symbol || !span_of(0, "/ld-linux", &own, &end) || !span_of(program, "/ld-linux", &its, &end))
    die("no dynamic loader in the program");
  errno = 0;
  long offset = ptrace(PTRACE_PEEKDATA, tid, its + (symbol - own), 0);
  if (errno) die("cannot read __rseq_offset");
  uint64_t thread_pointer;
  struct iovec iov = {&thread_pointer, sizeof thread_pointer};
  if (ptrace(PTRACE_GETREGSET, tid, NT_ARM_TLS, &iov) < 0) die("cannot read the thread pointer");
  uint64_t area = thread_pointer + offset;
  errno = 0;
  long ids = ptrace(PTRACE_PEEKDATA, tid, area + offsetof(struct rseq, cpu_id_start), 0);
  if (errno || (int)((uint64_t)ids >> 32) != cpu) die("the thread has no rseq area of glibc's");
  return area + offsetof(struct rseq, rseq_cs);
}

int main(int argc, char **argv) {
  if (argc < 7 || strcmp(argv[5], "--")) usage();
  uint64_t index = number(argv[2]), steps = number(argv[3]), warm_ms = number(argv[4]);
  char **command = argv + 6;
  FILE *out = fopen(argv[1], "we");
  if (!out) die("cannot write OUT");

  program = fork();
  if (program < 0) die("cannot fork");
  if (program == 0) {
    execv(command[0], command);
    perror(command[0]);
    _exit(127);
  }
  usleep(warm_ms * 1000);
  int status;
  if (waitpid(program, &status, WNOHANG) != 0) {
    program = 0;
    die("the program ended before its warm-up did");
  }

  static int tids[MAX_THREADS];
  int count = threads_of(program, tids);
  if (index >= (uint64_t)count) die("no such thread yet: give it a longer warm-up");
  pid_t tid = tids[index];
  cpu_set_t cpus;
  if (sched_getaffinity(tid, sizeof cpus, &cpus) || CPU_COUNT(&cpus) != 1)
    die("the thread is not pinned to one CPU: pin it (BENCH_CPUS) or give it a longer warm-up");
  int cpu = 0;
  while (!CPU_ISSET(cpu, &cpus)) cpu++;
  if (ptrace(PTRACE_ATTACH, tid, 0, 0) < 0) die("cannot attach");
  waitpid(tid, &status, __WALL);

  const char *name = strrchr(command[0], '/');
  name = name ? name : command[0];
  uint64_t base, end;
  if (!span_of(program, name, &base, &end)) die("the program is not mapped under its name");
  uint64_t cs = rseq_cs_of(tid, cpu);
  uint64_t *counts = calloc((end - base) / 4, sizeof *counts);
  if (!counts) die("no memory for the counts");

  uint64_t outside = 0, done = 0;
  while (done < steps) {
    struct user_regs_struct regs;
    struct iovec iov = {&regs, sizeof regs};
    if (ptrace(PTRACE_GETREGSET, tid, NT_PRSTATUS, &iov) < 0) break;
    if (regs.pc >= base && regs.pc < end)
      counts[(regs.pc - base) / 4]++;
    else
      outside++;
    done++;
    ptrace(PTRACE_POKEDATA, tid, cs, 0);
    int deliver = 0;
    for (;;) {
      if (ptrace(PTRACE_SINGLESTEP, tid, 0, deliver) < 0) goto stepped;
      waitpid(tid, &status, __WALL);
      if (!WIFSTOPPED(status)) goto stepped;
      if (WSTOPSIG(status) == SIGTRAP) break;
      // Another signal stopped it before the step: deliver it, but for a stop, and step again.
      deliver = WSTOPSIG(status) == SIGSTOP ? 0 : WSTOPSIG(status);
    }
  }
stepped:
  if (outside) fprintf(out, "0 %" PRIu64 "\n", outside);
  for (uint64_t i = 0; i < (end - base) / 4; i++)
    if (counts[i]) fprintf(out, "%" PRIx64 " %" PRIu64 "\n", i * 4, counts[i]);
  fclose(out);
  fprintf(stderr, "stepcount: %" PRIu64 " instructions of thread %d of %d\n", done, (int)index, count);
  kill(program, SIGKILL);
  waitpid(program, &status, 0);
  return done == steps ? 0 : 1;
}
