//! Builds C Lua 5.4 from the sources `LUA_SRC` names. Without them the build fails: every
//! comparison with C Lua must run, never be skipped.
//!
//! Built with `LUA_COMPAT_5_3`, as the stock makefile does. The sources are copied and one
//! thing changes: floats become text through a helper that
//! writes NaN as `nan`/`-nan` on every platform (MSVC's C library writes `-nan(ind)`;
//! glibc and RemedyLua write `-nan`), so comparisons do not trip over the C library.
//! luaconf.h also gets `luai_makeseed`: each new state takes a fresh string-hash seed
//! (`oracle_seed.c`), where stock C Lua's repeats within a process. And in lstate.c a new
//! coroutine's count hook fires at its first instruction instead of a full period later,
//! so the step limit (`run`) counts every coroutine: a fresh count per coroutine let work
//! spread over many short ones run unbounded.

use std::path::PathBuf;

const NUM2STR: &str = "#define lua_number2str(s,sz,n)  \\\n\tl_sprintf((s), sz, LUA_NUMBER_FMT, (LUAI_UACNUMBER)(n))";
const NUM2STR_FIX: &str =
    "int oracle_num2str(char *s, size_t sz, double n);\n#define lua_number2str(s,sz,n) oracle_num2str((s), (sz), (double)(n))";
const SEED_FIX: &str = "\nunsigned int oracle_seed(void);\n#define luai_makeseed(L) oracle_seed()\n";
const NEWTHREAD_HOOK: &str = "  L1->hook = L->hook;\n  resethookcount(L1);";
const NEWTHREAD_HOOK_FIX: &str = "  L1->hook = L->hook;\n  L1->hookcount = 1;  /* oracle: the step limit counts every coroutine */";
const FIX_C: &str = "#include <stdio.h>\n#include <math.h>\n\
int oracle_num2str(char *s, size_t sz, double n) {\n\
  if (n != n) return snprintf(s, sz, \"%s\", signbit(n) ? \"-nan\" : \"nan\");\n\
  return snprintf(s, sz, \"%.14g\", n);\n\
}\n";

fn main() {
    println!("cargo:rerun-if-env-changed=LUA_SRC");
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default());
    let src = PathBuf::from(std::env::var("LUA_SRC").unwrap_or_default());
    assert!(
        src.join("lua.h").is_file(),
        "\n\nremedy-lua-oracle: the C Lua 5.4.7 sources were not found at {}.\n\
         RemedyLua's tests and benchmarks compare every result with C Lua and do not run without it.\n\
         Set LUA_SRC to the directory holding lua.h and the other .c/.h files of Lua 5.4.7's src/,\n\
         e.g.  LUA_SRC=/path/to/lua-5.4.7/src cargo test -p remedy-lua\n",
        src.display()
    );
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap_or_default()).join("lua");
    let _ = std::fs::create_dir_all(&out);
    let mut files = Vec::new();
    for e in std::fs::read_dir(&src).into_iter().flatten().flatten() {
        let p = e.path();
        let Some(name) = p.file_name().and_then(|n| n.to_str()).map(str::to_owned) else { continue };
        let Ok(mut text) = std::fs::read_to_string(&p) else { continue };
        if name == "luaconf.h" {
            let fixed = text.replace("\r\n", "\n").replace(NUM2STR, NUM2STR_FIX);
            if !fixed.contains("oracle_num2str") {
                println!("cargo:warning=luaconf.h did not have the expected lua_number2str");
            }
            // Before the include guard's closing `#endif`.
            text = match fixed.rfind("#endif") {
                Some(i) => format!("{}{SEED_FIX}{}", &fixed[..i], &fixed[i..]),
                None => panic!("luaconf.h has no closing #endif"),
            };
        }
        if name == "lstate.c" {
            let fixed = text.replace("\r\n", "\n");
            assert!(fixed.contains(NEWTHREAD_HOOK), "lstate.c does not have the expected lua_newthread hook setup");
            text = fixed.replace(NEWTHREAD_HOOK, NEWTHREAD_HOOK_FIX);
        }
        let _ = std::fs::write(out.join(&name), text);
        if name.ends_with(".c") && name != "lua.c" && name != "luac.c" && name != "onelua.c" {
            files.push(out.join(&name));
        }
    }
    let fix = out.join("oracle_fix.c");
    let _ = std::fs::write(&fix, FIX_C);
    files.push(fix);
    // The thread CPU clock for timing tests (`thread_cpu`).
    files.push(manifest.join("oracle_cpu.c"));
    println!("cargo:rerun-if-changed=oracle_cpu.c");
    // The string-hash seed (`luai_makeseed`).
    files.push(manifest.join("oracle_seed.c"));
    println!("cargo:rerun-if-changed=oracle_seed.c");
    // The memory ceiling for `run` (`oracle_cap`).
    files.push(manifest.join("oracle_mem.c"));
    println!("cargo:rerun-if-changed=oracle_mem.c");
    // As the stock 5.4 makefile builds it (RemedyLua matches it by default; its strict mode
    // is tested against fixed expectations).
    cc::Build::new().define("LUA_COMPAT_5_3", None).include(&out).files(&files).warnings(false).opt_level(2).compile("lua54oracle");
    println!("cargo:rerun-if-changed={}", src.display());
}
