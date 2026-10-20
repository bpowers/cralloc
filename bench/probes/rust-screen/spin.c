// Spins one thread on each CPU of a list for a number of milliseconds, so that a run started
// straight after it finds the package at its boost clock rather than ramping from idle
// (bench/README.md, "Clock").
//
// Build: cc -O2 -pthread spin.c -o spin
// Use:   spin CPULIST MS        (CPULIST as taskset takes it, for example 0-3,8-11)
#define _GNU_SOURCE
#include <pthread.h>
#include <sched.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

static double ms_now(void) {
  struct timespec ts;
  clock_gettime(CLOCK_MONOTONIC, &ts);
  return ts.tv_sec * 1e3 + ts.tv_nsec / 1e6;
}

static double deadline;

static void *spin(void *arg) {
  cpu_set_t set;
  CPU_ZERO(&set);
  CPU_SET((int)(long)arg, &set);
  sched_setaffinity(0, sizeof set, &set);
  volatile unsigned long x = 0;
  while (ms_now() < deadline) {
    for (int i = 0; i < 10000; i++) x++;
  }
  return NULL;
}

int main(int argc, char **argv) {
  if (argc != 3) {
    fprintf(stderr, "usage: spin CPULIST MS\n");
    return 2;
  }
  int cpus[1024];
  int n = 0;
  char *list = strdup(argv[1]);
  for (char *tok = strtok(list, ","); tok != NULL && n < 1024; tok = strtok(NULL, ",")) {
    int a, b;
    if (sscanf(tok, "%d-%d", &a, &b) == 2) {
      for (int c = a; c <= b && n < 1024; c++) cpus[n++] = c;
    } else if (sscanf(tok, "%d", &a) == 1) {
      cpus[n++] = a;
    }
  }
  deadline = ms_now() + atof(argv[2]);
  pthread_t t[1024];
  for (int i = 0; i < n; i++) pthread_create(&t[i], NULL, spin, (void *)(long)cpus[i]);
  for (int i = 0; i < n; i++) pthread_join(t[i], NULL);
  return 0;
}
