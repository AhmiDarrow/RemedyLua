Idiomatic Lua the 21 benchmarks in `../lua` do not cover: metamethods, `__index`
functions, varargs, closures in loops, integer and bitwise arithmetic, `string.format`
logging, `utf8.*`, `string.byte`/`char` loops, `goto` state machines, deep recursion.
Each returns one value, compared with C Lua's:

    BENCH_DIR=benches/lua-extra BENCH_NO_WRITE=1 cargo run --release --example benchsuite -p remedy-lua
