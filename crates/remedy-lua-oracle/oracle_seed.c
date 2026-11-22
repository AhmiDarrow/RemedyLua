/* C Lua's string-hash seed (`luai_makeseed`, defined in luaconf.h to call this). Stock C
 * Lua mixes time(NULL) with addresses, which within one test process repeat: every state
 * made in the same second got the same seed, so rerunning a program never showed the
 * other `pairs`/`next` orders C Lua gives across processes. Here each new state on a
 * thread takes the next value of a sequence started from the time and an address, as
 * random as C Lua's own seed and different for every state. */
#include <stddef.h>
#include <time.h>

#if defined(_MSC_VER)
#define ORACLE_TLS __declspec(thread)
#else
#define ORACLE_TLS _Thread_local
#endif

static ORACLE_TLS unsigned long long next_seed;
static ORACLE_TLS int seeded;

unsigned int oracle_seed(void) {
  if (!seeded) {
    next_seed = (unsigned long long)time(NULL) ^ (unsigned long long)(size_t)&next_seed;
    seeded = 1;
  }
  next_seed = next_seed * 6364136223846793005ull + 1442695040888963407ull;
  return (unsigned int)(next_seed >> 32);
}
