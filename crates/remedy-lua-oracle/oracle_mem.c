/* A ceiling on C Lua's memory for `run`: C Lua has no memory limit of its own, and a
 * fuzzed program that doubles a string by concatenation would otherwise take the whole
 * test process (and the machine's memory) with it. The allocator counts the bytes the
 * state holds, refuses growth past the cap, and remembers that it did, so the caller can
 * report the run as the oracle's own limit (unforgeable from Lua, unlike a message). */
#include <stdlib.h>
#include "lua.h"

typedef struct { long long used, cap; int hit; } OracleMem;

static void *oracle_alloc(void *ud, void *p, size_t os, size_t ns) {
  OracleMem *m = (OracleMem *)ud;
  long long old = p ? (long long)os : 0;
  void *q;
  if (ns == 0) {
    free(p);
    m->used -= old;
    return NULL;
  }
  if ((long long)ns > old && m->used + ((long long)ns - old) > m->cap) {
    m->hit = 1;
    return NULL;
  }
  q = realloc(p, ns);
  if (q) m->used += (long long)ns - old;
  else m->hit = 1;
  return q;
}

/* Installs the capped allocator on a state made by luaL_newstate (same realloc/free, so
 * blocks it already holds are freed correctly); 0 if out of host memory. */
int oracle_cap(lua_State *L, long long cap) {
  OracleMem *m = (OracleMem *)malloc(sizeof *m);
  if (!m) return 0;
  m->used = (long long)lua_gc(L, LUA_GCCOUNT, 0) * 1024 + lua_gc(L, LUA_GCCOUNTB, 0);
  m->cap = cap;
  m->hit = 0;
  lua_setallocf(L, oracle_alloc, m);
  return 1;
}

/* Whether the capped allocator refused an allocation since `oracle_cap`. */
int oracle_cap_hit(lua_State *L) {
  void *ud = NULL;
  lua_Alloc f = lua_getallocf(L, &ud);
  return f == oracle_alloc && ud && ((OracleMem *)ud)->hit;
}

/* lua_close, then frees the counter if the state had one. */
void oracle_close(lua_State *L) {
  void *ud = NULL;
  lua_Alloc f = lua_getallocf(L, &ud);
  lua_close(L);
  if (f == oracle_alloc) free(ud);
}
