# Profiling RemedyLua on Windows

A sampling profiler that needs no admin rights and no installed profiler. `sampler.py` runs
a program, samples its main thread's instruction pointer about every half millisecond
(SuspendThread/GetThreadContext through Python ctypes), and maps each sample to a function
using the linker's map file. `annotate.py` then shows the hottest instructions of one
function, using `llvm-objdump`.

1. Build the program with a linker map, in a separate target directory so your normal
   build cache is not invalidated:

   ```sh
   CARGO_TARGET_DIR=../target-prof RUSTFLAGS="-C link-arg=/MAP:$PWD/../target-prof/prof.map" \
     cargo build --release -p remedy-lua --example prof
   ```

2. Sample a run. The arguments after the map are passed to the program (`prof` takes a
   Lua file and a run count):

   ```sh
   python tools/profile/sampler.py ../target-prof/release/examples/prof.exe ../target-prof/prof.map \
     crates/remedy-lua/benches/lua/fib.lua 8
   ```

   It prints the program's output, then the share of samples per function. `prof` also
   times C Lua on the same file, so C Lua's functions (`luaV_execute`...) appear too.
   The raw addresses go to `tools/profile/sampler.py.raw`.

3. Annotate one function, here the narrow fast loop, showing the 30 hottest spots:

   ```sh
   python tools/profile/annotate.py ../target-prof/release/examples/prof.exe ../target-prof/prof.map \
     tools/profile/sampler.py.raw fast3runKj100 30
   ```

Delete the extra target directory when you are done.
