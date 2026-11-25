/* Where C Lua's answer is its C library's text, Windows' library is made to write what
 * glibc (the reference) writes, so a comparison measures Lua and not the library:
 * pointers as `0x` and lowercase hex, infinities and NaN as `inf` and `nan` whatever the
 * flags, files in binary mode, and local time as UTC whatever the machine's zone. Nothing
 * here changes on other systems. */
#ifdef _WIN32
#include <fcntl.h>
#include <locale.h>
#include <math.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

/* Once, before the first state. */
void oracle_init(void) {
  _putenv_s("TZ", "UTC");
  _tzset();
  _set_fmode(_O_BINARY);
}

/* loslib.c's setlocale: "POSIX" names the "C" locale (the library here does not know it). */
char *oracle_setlocale(int cat, const char *l) {
  return setlocale(cat, l != NULL && strcmp(l, "POSIX") == 0 ? "C" : l);
}

/* Twelve digits, as addresses on the reference system have: a padded address is then
 * as long in both. */
int oracle_ptr2str(char *s, size_t sz, const void *p) {
  return snprintf(s, sz, "0x%012llx", (unsigned long long)(size_t)p);
}

/* lstrlib.c's sprintf (luaconf.h builds Windows as C89, so `l_sprintf` is sprintf and
 * `sz` here is only a bound past any of its buffers) and its `lua_number2strx`: one
 * conversion a call, the conversion's letter last. */
int oracle_fmt(char *s, size_t sz, const char *fmt, ...) {
  va_list ap;
  size_t n = strlen(fmt);
  char conv = n >= 2 && n < 60 && fmt[0] == '%' ? fmt[n - 1] : '\0';
  int r;
  va_start(ap, fmt);
  if (conv == 'p') {
    char f[64], a[32];
    memcpy(f, fmt, n + 1);
    f[n - 1] = 's';
    oracle_ptr2str(a, sizeof a, va_arg(ap, void *));
    r = snprintf(s, sz, f, a);
  } else if (conv != '\0' && strchr("aAeEfFgG", conv) != NULL) {
    const char *p = fmt + 1;
    int left = 0, plus = 0, space = 0, alt = 0, zero = 0, width, hex = conv == 'a' || conv == 'A';
    va_list aq;
    double x;
    va_copy(aq, ap);
    x = va_arg(aq, double);
    va_end(aq);
    for (; strchr("-+ #0", *p) != NULL; p++) {
      left |= *p == '-';
      plus |= *p == '+';
      space |= *p == ' ';
      alt |= *p == '#';
      zero |= *p == '0';
    }
    width = atoi(p);
    if (x != x || x - x != 0 || hex) {
      /* Not finite: the sign and the name, padded with spaces (the library here writes
       * "-nan(ind)", and "i.nf" under `#`). A hexadecimal float: the digits it has and
       * no more when no precision is given (the library here always writes thirteen). */
      char body[160];
      const char *sign = signbit(x) ? "-" : plus ? "+" : space ? " " : "";
      int pad;
      if (x != x || x - x != 0) {
        snprintf(body, sizeof body, "%s", x != x ? (conv < 'a' ? "NAN" : "nan") : (conv < 'a' ? "INF" : "inf"));
        zero = 0;
      } else {
        const char *dot = strchr(p, '.');
        char f[8] = "%#.*a";
        char *e, *d;
        f[4] = conv;
        if (!alt) memmove(f + 1, f + 2, 5);
        snprintf(body, sizeof body, f, dot != NULL ? atoi(dot + 1) : 13, fabs(x));
        e = strchr(body, conv == 'a' ? 'p' : 'P');
        if (dot == NULL && e != NULL) {
          for (d = e; d[-1] == '0'; d--) {}
          if (d[-1] == '.' && !alt) d--;
          memmove(d, e, strlen(e) + 1);
        }
      }
      pad = width - (int)(strlen(sign) + strlen(body));
      if (pad < 0) pad = 0;
      if (left) r = snprintf(s, sz, "%s%s%*s", sign, body, pad, "");
      else if (zero) r = snprintf(s, sz, "%s%.2s%0*d%s", sign, body, pad, 0, body + 2);
      else r = snprintf(s, sz, "%*s%s%s", pad, "", sign, body);
      if (zero && pad == 0) r = snprintf(s, sz, "%s%s", sign, body);
    } else {
      r = vsnprintf(s, sz, fmt, ap);
    }
  } else {
    r = vsnprintf(s, sz, fmt, ap);
  }
  va_end(ap);
  return r;
}
#else
void oracle_init(void) {}
#endif
