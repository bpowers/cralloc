// Runs a command and writes its wall time in seconds to a file. perf stat notices that its
// child has exited only at its next poll, which adds 10 to 50 ms to a wall time taken outside it,
// so screen.py takes the time here, inside perf stat.
//
// Build: cc -O2 walltime.c -o walltime
// Use:   walltime OUTFILE command args...      exits with the command's status
#include <stdio.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

int main(int argc, char **argv) {
  if (argc < 3) {
    fprintf(stderr, "usage: walltime OUTFILE command args...\n");
    return 2;
  }
  struct timespec a, b;
  clock_gettime(CLOCK_MONOTONIC, &a);
  pid_t pid = fork();
  if (pid == 0) {
    execvp(argv[2], argv + 2);
    perror("walltime: exec");
    _exit(127);
  }
  int status = 0;
  waitpid(pid, &status, 0);
  clock_gettime(CLOCK_MONOTONIC, &b);
  FILE *f = fopen(argv[1], "w");
  if (f != NULL) {
    fprintf(f, "%.6f\n", (b.tv_sec - a.tv_sec) + (b.tv_nsec - a.tv_nsec) / 1e9);
    fclose(f);
  }
  if (WIFEXITED(status)) return WEXITSTATUS(status);
  return 128 + WTERMSIG(status);
}
