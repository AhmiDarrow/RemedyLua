/* The calling thread's CPU time in nanoseconds, for timing tests that must not count
 * other processes' load. On Windows it is the thread's cycle count (QueryThreadCycleTime;
 * GetThreadTimes only moves every ~15.6 ms tick, too coarse for millisecond runs) scaled
 * by the cycle rate, measured once against the performance counter over 10 ms. */
#ifdef _WIN32
#include <windows.h>
unsigned long long oracle_thread_cpu(void) {
  static double ns_per_cycle = 0;
  ULONG64 c = 0;
  if (ns_per_cycle == 0) {
    LARGE_INTEGER f, a, b;
    ULONG64 c0, c1;
    QueryPerformanceFrequency(&f);
    QueryPerformanceCounter(&a);
    c0 = ReadTimeStampCounter();
    do QueryPerformanceCounter(&b); while ((b.QuadPart - a.QuadPart) * 100 < f.QuadPart);
    c1 = ReadTimeStampCounter();
    ns_per_cycle = (double)(b.QuadPart - a.QuadPart) * 1e9 / (double)f.QuadPart / (double)(c1 - c0);
  }
  QueryThreadCycleTime(GetCurrentThread(), &c);
  return (unsigned long long)((double)c * ns_per_cycle);
}
#else
#include <time.h>
unsigned long long oracle_thread_cpu(void) {
  struct timespec t;
  if (clock_gettime(CLOCK_THREAD_CPUTIME_ID, &t) != 0) return 0;
  return (unsigned long long)t.tv_sec * 1000000000ull + (unsigned long long)t.tv_nsec;
}
#endif

/* The process's resident memory now and at its peak, in bytes, where /proc does not say
 * (zeros elsewhere): for tests that bound what a script makes resident. */
#ifdef _WIN32
#include <psapi.h>
void oracle_process_memory(unsigned long long *now, unsigned long long *peak) {
  PROCESS_MEMORY_COUNTERS c;
  c.cb = sizeof c;
  *now = *peak = 0;
  if (K32GetProcessMemoryInfo(GetCurrentProcess(), &c, sizeof c)) {
    *now = c.WorkingSetSize;
    *peak = c.PeakWorkingSetSize;
  }
}
#else
void oracle_process_memory(unsigned long long *now, unsigned long long *peak) {
  *now = *peak = 0;
}
#endif
