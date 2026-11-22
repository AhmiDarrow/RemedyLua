/* The calling thread's CPU time, for timing tests that must not count other processes'
 * load: cycles on Windows (QueryThreadCycleTime; GetThreadTimes only moves every ~15.6 ms
 * tick, too coarse for millisecond runs), nanoseconds elsewhere. */
#ifdef _WIN32
#include <windows.h>
unsigned long long oracle_thread_cpu(void) {
  ULONG64 c = 0;
  QueryThreadCycleTime(GetCurrentThread(), &c);
  return c;
}
#else
#include <time.h>
unsigned long long oracle_thread_cpu(void) {
  struct timespec t;
  if (clock_gettime(CLOCK_THREAD_CPUTIME_ID, &t) != 0) return 0;
  return (unsigned long long)t.tv_sec * 1000000000ull + (unsigned long long)t.tv_nsec;
}
#endif
