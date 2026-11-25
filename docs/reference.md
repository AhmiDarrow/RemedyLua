# RemedyLua reference

RemedyLua is Lua 5.4 written from scratch in safe Rust, built to run other people's code
inside an application. This file is its reference. It covers:

1. [The RemedyLua dialect](#the-remedylua-dialect): type annotations, compound assignment,
   `continue` and string interpolation, in `.rlua` files.
2. [The type checker](#the-type-checker): its design, what it checks and what it does not,
   its messages and its bounds.
3. [Beyond Lua 5.4](#beyond-lua-54): where RemedyLua differs from C Lua 5.4.7. It is a
   strict superset: C Lua's implementation limits are gone, and a few behaviours are
   configurable.
4. [Hashing seed and replays](#hashing-seed-and-replays) and
   [the length of a table with holes](#the-length-of-a-table-with-holes).
5. [The standard library](#the-standard-library): which libraries exist, which `Libs`
   flag gives each, and every function's RemedyLua-specific behaviour, limits and costs.
6. [Budgets](#budgets): `Limits`, fuel, memory and the other bounds, and what happens at
   each.
7. [Embedding guide](#embedding-guide): the host API (states, calls, host functions,
   userdata, data, output, diagnostics, interrupts, WebAssembly) and an
   [index of the API](#api-index).

The Lua 5.4 manual remains the reference for everything this file does not mention:
plain Lua programs behave exactly as they do in C Lua 5.4.7.

## The RemedyLua dialect

### Choosing it

A chunk is RemedyLua (the dialect) when its name ends in `.rlua`, ignoring case:

- `Lua::load(src, "=widget.rlua")` and `Lua::exec(src, "@ui/list.rlua")`. A leading `=`
  or `@` is stripped before the extension is looked at.
- `load(src, "=x.rlua")` from a script (with `Libs::LOAD`).
- `remedy_lua::check(src, "x.rlua")` and `remedy_lua::format(src, "x.rlua")`.
- `Lua::load_with(src, name, Dialect::RemedyLua)` chooses it whatever the name.

Every other name, `=(load)` included, is plain Lua 5.4 (`Dialect::Lua54`).

The dialect is a superset of plain Lua. Every plain Lua program means the same thing in a
`.rlua` chunk. The new words (`type`, `continue`) are keywords only where plain Lua could
not use a name there, so a program that uses them as variable names still works. Every
addition compiles to the plain Lua it stands for. Annotations and `type` statements
compile to nothing at all, so a chunk with annotations runs exactly as it does without them,
with the same results, errors, fuel and memory. The test suite checks this on every
program it has both ways (`tests/rlua_types.rs`, `sweep1_fuzz::fuzz_rlua`).

### Type annotations

Annotations go after names, with a colon:

```lua
local count: integer = 0
local name: string?, age: number? = nil, nil
local function area(w: number, h: number): number return w * h end
local log = function(fmt: string, ...: any): () print(fmt:format(...)) end
for i: integer = 1, 10 do end
for key: string, value: number in pairs(scores) do end
local handle <close>: any = nil           -- attributes come before the colon
```

- **Locals**: `local x: T`, for each name of a `local` statement.
- **Parameters and varargs**: `function f(a: T, ...: U)`. `self` (in `function t:m()`)
  has no annotation of its own.
- **Results**: `function f(): T`, `function f(): (T, U)`, or `function f(): ()` for
  "returns nothing". Without one, a function's results are not checked.
- **Loop variables**: `for i: T = ...` and `for k: T, v: U in ...`.
- **Type aliases**: `type Name = T`, a statement. It defines `Name` for the whole block it
  is in, so aliases may refer to each other and to themselves (recursive types). It
  compiles to nothing.

The type syntax:

| Form | Meaning |
|---|---|
| `any` | Any value. Unchecked both ways (see [the type checker](#the-type-checker)). |
| `nil`, `boolean`, `number`, `integer`, `string`, `function`, `table`, `userdata`, `thread` | The values of that Lua type. `integer` is the integers (`math.type(x) == "integer"`). Every integer is a `number`. `function` is any function, and `table` any table. |
| `"left"`, `'right'` | That one string. |
| `T?` | `T` or `nil`. |
| `T \| U \| ...` | A union: any of them. `?` binds tighter than `\|`. |
| `{T}` | An array: a table whose integer keys hold `T`s. |
| `{[K]: V}` | A map: a table whose keys are `K`s and whose values are `V`s. |
| `{x: T, y: U}` | A record (shape): a table with exactly these fields. `,` or `;` separates them. `{}` is the record with no fields. |
| `(T, U) -> R`, `(T, ...) -> (R, S)`, `() -> ()` | A function type. Parameter names are allowed and ignored (`(x: number) -> ()`). `...` last means variadic. The results are one type, a list in parentheses, or `()`. |
| `(T)` | `T` (parentheses group). |
| `Name` | An alias defined by `type Name = ...` in this block or an enclosing one. |
| `a.b.Name` | A host's or module's type. Allowed, and not known to the checker: it means `any`. |

`type` is a keyword only as `type Name = ...` at the start of a statement, so `type(x)`
and `local type = 1` still work.

### Compound assignment

`a op= b` assigns `a op b` to `a`, for `+= -= *= /= //= %= ..=`:

```lua
local n = 0
n += 1
t.count //= 2
t[key()] ..= "!"     -- key() is called once
```

- The target is a name, a field or an indexed table (as for `=`). Its parts (the table and
  the key) are evaluated once.
- The operator and its metamethods are the plain operator's (`__add` for `+=`, `__concat`
  for `..=`, and so on).
- It is a statement, not an expression, and takes one target and one value.
- There is no `^=`, and no bitwise forms.

### `continue`

`continue` inside a loop goes to the loop's next iteration:

- In `while` and `for` loops, to the next test of the condition, or the next step.
- In `repeat ... until cond`, to the `until` test, which sees the body's locals, as it
  always does.

It is a statement only where a statement starts and the next token cannot continue an
expression. So `continue = 1`, `continue(x)` and `local continue` still use a variable
named `continue`. It must be inside a loop, and inside the same function.

### String interpolation

A backtick string interpolates the expressions in braces:

```lua
local s = `{name} has {#items} items, total {total * 1.2}`
```

- Each `{expr}` is converted as `tostring` converts it: numbers as Lua prints them, `nil`
  as `nil`, and tables through `__tostring` and `__name`. The conversion is built in, so
  replacing the global `tostring` does not change it. The pieces are then concatenated.
- `\{` and `\}` write braces. The other escapes are those of `"..."` strings. A backtick
  string may span lines without a backslash; the line breaks are part of it.
- Interpolations nest (`` `a {`b {c}`} ` ``).
- An empty or invalid expression in braces, or an unclosed one, is a syntax error. A
  backtick string is one token, so a call needs parentheses (`` f(`x`) ``, not
  `` f`x` ``).

`continue` outside a loop is an error when the chunk is compiled ("continue outside loop
at line N").

## The type checker

`remedy_lua::check(source, name)` parses a chunk without running it and, for a `.rlua`
chunk, checks its annotations. It returns `Diagnostic`s (see
[Diagnostics](#diagnostics)). Checking is separate from running: `load`, `exec` and the
rest never check types, and nothing the checker decides reaches a running program. A
`.rlua` program runs exactly as its plain-Lua desugaring does, however wrong its
annotations are.

### Design: gradual, and sound where it reports

- **Unannotated code is dynamic.** A global, an unannotated parameter, a field of an
  unannotated table, the result of a call to a function without result annotations, and
  every standard library function are `any`. Globals are `any` even when they hold library
  functions, because a script or host can replace them. An `any` value can be used in
  every way and goes everywhere. A plain Lua program gets no type diagnostic, whatever it
  does: the suite checks every corpus, spec and benchmark program, and generated ones, as
  `.rlua`.
- **Annotated code is checked.** Where an annotation stands (a local, a parameter, a
  result, a loop variable, a record field reached through an annotated value), what goes
  there must fit it. An operator, call or index whose operand's type comes from an
  annotation is checked against what that type's values can do.
- **Sound where it reports.** The rules below are those of a sound type system: tables'
  contents are invariant, parameters contravariant and results covariant. An operation is
  reported when some value of its operands' types makes it fail at run time. A `T?` value
  may be `nil`, so indexing it is reported until a condition rules `nil` out.
- **What is not guaranteed.** There are no run-time checks, so an `any` value that does
  not fit an annotation is not caught where it enters typed code. Typed code is only as
  right as the dynamic values it is given. The checker also trusts that the standard
  `error` and `assert` are the global ones (for "this call does not return" and "this is
  true after `assert(x)`"), and that strings' metatable is the standard one.

### Types and where they come from

- **From annotations**: see [the type syntax](#type-annotations).
- **From literals**: `1` is `integer`, `1.5` is `number`, `"x"` the string `"x"`, `true`
  and `false` themselves, `nil`, a function literal its annotated signature, and a table
  constructor a fresh table of its fields' types.
- **From operators**: arithmetic on integers is `integer`, except `/` and `^`, which are
  `number`. With a float it is `number`, and with a string operand (Lua's coercion) also
  `number`. Bitwise operators give `integer`; `..` gives `string`; comparisons and `not`
  give `boolean`; `#` of a string is `integer`. `a and b` and `a or b` give what Lua gives:
  for `x or 0` with `x: number?`, `number`. An operator with an operand that may have a
  metatable (a table, userdata or `any`) gives `any`, as the metamethod decides.
- **From locals**: an annotated local has its annotation. An unannotated local that is
  never assigned after its declaration has the type of its initializer, when that type came
  from annotations (`local v = arr[i]` with `arr: {T}` is `T?`). Otherwise it is `any`: an
  unannotated `local x = 1` stays dynamic.
- **Reading**: `r.f` of a record is the field's type. A field the record does not have is an
  error. `a[i]` of an array `{T}` is `T?`, because the index may be past the end; `m[k]`
  of a map `{[K]: V}` is `V?`. Use `a[i] or default`, or a condition, to drop the `nil`. A
  call of a typed function gives its results.

### What fits where (subtyping)

`S` fits `T` (a value of type `S` may go where `T` is expected) when:

- either is `any`;
- `integer` fits `number`, a string literal fits `string`, `true` and `false` fit
  `boolean`, and every type fits itself;
- `S` is a union and each of its members fits `T`, or `T` is a union and `S` fits one of
  its members;
- `{S}` fits `{T}` and `{[K1]: V1}` fits `{[K2]: V2}` only when the contents are the same
  both ways (invariant: the table can be written through either view). An array fits a
  map with `integer` or `number` keys, and the other way, the same way;
- a record fits a record when it has every field the other has, each with the same type
  both ways (it may have more);
- a function type fits another when the other's parameters fit its own (contravariant: a
  function that takes `number?` can stand for one that takes `number`) and its results fit
  the other's (covariant). A function that takes fewer parameters fits one that is passed
  more (Lua drops the extra arguments). Missing arguments and results are `nil`. A
  function without result annotations fits any results;
- `function` and `table` are "some function" and "some table": every function type and
  every table type fits them, and they fit every function type and table type (gradually,
  like `any`);
- **a table constructor is fresh**: no other reference to it exists, so it is checked
  covariantly against what it is stored as. `{1, 2}` fits `{number}`, `{x = 1}` fits
  `{x: number, y: string?}` (a missing field is `nil`), and `{a = 1}` fits
  `{[string]: number}`. Nested constructors are fresh too. A table that is not a
  constructor (a local holding one) is held to the invariant rules.

Recursive types (`type List = {value: number, next: List?}`) are compared as infinite
trees: a comparison already under way is assumed to hold while it is decided. A plain
alias cycle with no structure (`type A = B; type B = A`) is an error.

### What is checked

Each is a `Severity::Error` diagnostic. The messages are in the form shown:

| Check | Message |
|---|---|
| A value assigned to an annotated local, at its declaration or later (compound assignment included) | `local 'x': expected number, got string` |
| An argument to a typed function (a local function, a function-typed field or local) | `argument #1 to 'f': expected number, got "a"` |
| Too many or too few arguments for a non-variadic typed function | `too many arguments to 'f': expected 1, got 2`, `missing argument #2 to 'f' (expected string)` |
| `self` of a method call (`o:m()`) | `self of method 'm': expected ..., got ...` |
| A returned value, the count included | `return value #1: expected string, got number`, `too many return values: expected 0, got 1`, `missing return value #2 (expected string)` |
| A function with non-optional declared results whose end can be reached | `function may end without returning number` |
| A field written through a record, array or map | `field 'x': expected number, got string`, `array element: expected number?, got string`, `map value: ...` |
| A field a record does not have, read or written | `no field 'z' in {x: number}` |
| A key of the wrong type for an array or map | `field 'n' is not a valid index for {number}`, `this key is not a valid key for {[string]: number}` |
| An annotated loop variable | `loop variable 'i': expected string, got integer` |
| A typed numeric `for` bound that is not a number | `'for' initial value must be a number` (also `limit`, `step`) |
| Indexing, calling, arithmetic, bitwise operations, `..`, `#` and ordering on a typed value that may not support them | `attempt to index a nil value (local 't')`, `attempt to call a number value (local 'x')`, `attempt to index a value that may be nil (local 't' is {x: number}?)`, `attempt to compare number with string` |
| An unknown type name | `unknown type 'Foo'` |
| An alias defined twice in a block, a built-in redefined, a plain alias cycle | `type 'T' is already defined in this block`, `cannot redefine the built-in type 'number'`, `type 'T' is defined only in terms of itself` |

An operation is checked only when one of its operands' types came from an annotation. It
is not reported when the other operand may supply a metamethod (a table, userdata or
`any`), or when the value's type includes `any`. Ordering (`<`) of a number with a string,
or of `nil`, fails unless a metamethod can help. Arithmetic on a string literal that is not
a number (`x + "abc"`) is reported, while one that is (`x + "10"`) is not.

"May end without returning" counts as ending: falling off the last statement, a `goto`,
a loop that can stop, or an `if` without an `else`. A block ends without falling through
when it ends in `return`, `break`, `continue`, a call of the global `error`, an
`if`/`else` all of whose branches do, `do ... end` around one of these, or a
`while true do` (or `repeat ... until false`) loop without a `break`.

### Narrowing

Inside a condition's reach, a local's type is narrowed:

- `if x then`, `while x do`, `x and ...`: `x` is not `nil` or `false`. `if not x then`,
  `x or ...`: the reverse. `x ~= nil` and `x == nil` (either way round) too, through `not`,
  `and` and `or` in the condition (up to 64 parts of it).
- **Early exits**: after `if not x then return end` (or `error`, `break`, `continue`), and
  after `assert(x)`, `x` is not `nil` for the rest of the block. An `if` whose `else` exits
  narrows the other way.
- **Assignment**: after `x = v` with a `v` that cannot be `nil`, an annotated `x: T?` is
  `T`, until the next assignment or the end of the block the assignment is in.

What may undo a narrowing ends it:

- A local never assigned after its declaration keeps its narrowing everywhere in reach,
  closures included.
- A local that is assigned keeps a narrowing until the next assignment to it. A loop
  around the narrowing that assigns it, or a label (a `goto` may come from anywhere), ends
  the narrowing there. A function written inside the narrowed part does not see it
  (it may run later).
- A local assigned from inside a nested function is never narrowed: any call might change
  it. Copy it to a local first (`local f = callback if f then f() end`).

Fields are not narrowed: `if t.x then t.x.y end` is reported when `t.x` is optional. Use a
local.

### What the checker does not do

- No generics, no casts, no type of `self` other than what its object has, and no
  inference of unannotated functions' results. `any` is the escape hatch.
- No checking of library calls: library functions are globals, so `any`.
- Records are exact for fields the checker knows. A record value with a metatable that
  gives it methods must list those methods as fields (`len: (self: Point) -> number`).
- Unions of more than one function signature are callable, but their arguments are not
  checked.
- No join of narrowings: after `if c then x = 1 else x = 2 end`, `x` is its declared type
  again.

### Bounds

`check` runs on other people's code, so it is bounded like the rest of the front end:

- **No recursion that follows the program's nesting.** The walk over statements and
  expressions, the conversion of annotations, subtyping, the narrowing of conditions, the
  reachability analysis and the printing of types use explicit stacks. 100,000-deep
  programs, types, aliases and conditions are tested on a 64 KiB thread.
- **Work.** The check does at most 4 million plus 128 per source byte steps, about linear
  in the source. Subtyping remembers what it decided, so types that share parts are not
  re-compared. Messages print types to about 120 characters.
- **Memory.** Its own tables are counted and stay within the memory a check's parse may use
  (`1 KiB per source byte + 64 MiB`).
- **Diagnostics.** At most 500.

Past the work or memory bound, the check stops. What it found before is kept, and one
`Severity::Warning` is added at the statement where it stopped: "type checking stopped
here: the program is too large or its types too complex for the work a check may do; what
follows was not checked" (or "...: not enough memory; ..."). A stopped check reports
nothing it is unsure of.

The checker runs in two passes over the same tree. The first finds which locals are
assigned after their declaration, from where, and which loops contain a `break`. The
second checks. `tests/rlua_types.rs` has the expected diagnostics of about 110 typed
programs, the no-false-positive run over the corpora, the behaviour of annotated programs
against their plain twins, the deep and wide inputs, and a fuzz of `check` on mutated
`.rlua` programs (`RLUA_CHECK_FUZZ_ITERS`, `RLUA_CHECK_FUZZ_SEED`).

## Beyond Lua 5.4

RemedyLua is a strict superset of Lua 5.4:

- Every program that works in C Lua 5.4 behaves the same here.
- A program that C Lua rejects only because of one of its implementation limits runs
  here.
- The only limits are the host's budgets: fuel, memory and output (see `Limits`), and a
  call depth a host may set. Hitting fuel, memory, the interrupt or the time limit stops
  the script with `Error::Limit`, which no script can catch. Output past `Limits::output`
  is dropped and the script runs on (see [Budgets](#budgets)).
- Recursion is bounded by memory, like the value stack: call frames are charged to the
  memory budget as they grow, and `Limits::call_depth` is unlimited by default. Memory
  that runs out where C Lua would already have stopped with its own error is that error,
  which `pcall` can catch: past C Lua's 200 C levels (metamethods, protected calls and
  resumes nested) it is `C stack overflow`; past its 1,000,000 stack slots, or with the
  thread's own call stack filling the budget, it is `stack overflow`. The frames are given
  back as the error unwinds. (Memory full of anything else is the memory limit.) So recursion C Lua runs runs
  here given the memory (C Lua allows up to about a million frames of the smallest
  functions: 136 bytes a frame plus its registers here, so a million-deep recursion runs
  within a 256 MiB budget), and
  unbounded recursion stops with C Lua's error. Lua-to-Lua calls use no native stack. A
  host may still set `call_depth`: deeper recursion is then the same `stack overflow`.

C Lua's fixed limits, and what RemedyLua does instead:

| C Lua 5.4 limit | C Lua message | RemedyLua |
|---|---|---|
| 200 local variables per function | `too many local variables (limit is 200)` | No limit of its own (locals and parameters are registers: below). |
| 255 registers per function (locals plus temporaries; calls with ~250+ arguments) | `function or expression needs too many registers` | Functions past 255 registers compile to wide instructions and run in a wide interpreter loop; functions that fit in 8 bits keep the fast form, compiled exactly as before. Past 32,767 registers, instructions reach theirs through windows (`Window`, `MoveAbs`), and counts past 16 bits (a call with 70,000 arguments, a `return` or `local` list as long) go through `SetTop` and `PadNil`. Registers are 32-bit numbers, so a function may have about four billion: the memory budget (each register is 16 bytes of stack while the function runs, and its source and bytecode are charged as they are made) stops a function long before that. 70,000-register functions are tested. |
| 255 upvalues per function | `too many upvalues` | Up to 65,535. |
| ~200 nested syntax levels (parentheses, blocks, functions, table constructors, call chains, `elseif` chains), and the C stack | `chunk has too many syntax levels`, `C stack overflow` | No fixed level. The parser, compiler and syntax-tree drop do not recurse: their explicit stacks are charged to the memory budget, so nesting stops only at an exact memory error. 100,000-deep programs are tested. |
| 200 C levels (`LUAI_MAXCCALLS`): protected calls nested in each other (`pcall(pcall, ..., f)`, or a `pcall` whose callee calls `pcall`), resumes inside resumes | `C stack overflow` (and `error in error handling` from 220) | Protected calls one inside the other run as one frame (`pcall(pcall, f)`) or as frames of their own, using no native stack, so any depth runs (20,000 levels are tested on a 256 KiB stack). Nested resumes stop where C Lua's do: a resume is refused when the resuming thread holds C Lua's 200 C levels, counted as C Lua counts them (so protected calls past the limit leave no room for a resume), and at most `Limits::native_depth` resumes nest. When C Lua's 200th level is reached by a protected call rather than a resume, C Lua's `pcall` refuses it with an unpositioned "C stack overflow"; here that call runs and the next resume is refused (positioned at a Lua caller of `wrap`). A message handler that keeps failing is called again as in C Lua, with "C stack overflow" at C Lua's 200th level and "error in error handling" at its 220th, counted the way C Lua counts its C levels (each call made where the handler's error was raised, so an error from inside a metamethod or a library callback costs more levels a round; the overflow positioned at the Lua function whose metamethod call reached the 200th); deeper than C Lua's limit, the handler runs as at C Lua's last level (once with the error, then with "C stack overflow", up to 20 times in all). |
| ~1,000,000 stack slots (`LUAI_MAXSTACK`) | `too many results to unpack`, `string slice too long`, `stack overflow` | The value stack is bounded only by the memory budget (`Limits::stack_slots` is unlimited by default). `table.unpack`, `string.byte` and `utf8.codepoint` charge the memory for their results before building them. |
| Operator chains of any length (C Lua recurses on long `..` / `^` chains) | `C stack overflow` | Compiled in a loop; bounded by memory. |

What stays the same:

- Errors that are part of the language: type errors, `'for' step is zero`, and the like.
- Every library function's behaviour.
- Error messages and positions.
- Lua's batch-of-50 table constructor flush, which remains only as an implementation detail.

Where C Lua 5.4.7 is wrong, RemedyLua does what the language says:

- A `goto` that leaves a block holding a to-be-closed variable and jumps to a label past a
  local with a run-time value (`do local c <close> = x if p then goto cont end end local v
  = {} ::cont::`, typically as a loop's `continue`). C Lua 5.4.7 compiles that jump wrong:
  depending on the shape it runs the `__close` handlers once instead of on every pass,
  leaves the loop early, fails with "attempt to call a nil value (metamethod 'close')", or
  crashes. RemedyLua closes the variable each time the `goto` leaves its block, as the
  manual's rule for to-be-closed variables says (`parity_26` pins the output).
- A comparison used as the key into an upvalue table (`local t = {} local function f()
  t[1 < 2] = 1 end`, or reading `t[a == b]`). C Lua 5.4.7 compiles the key wrong and fails
  with "attempt to index a number value"; RemedyLua indexes with the boolean, as the manual
  requires (`parity_28`).

### Lua 5.3 compatibility (on by default) and strict 5.4

Stock C Lua 5.4 is built with `LUA_COMPAT_5_3` (its makefile defines it), and RemedyLua
matches that build. The flag enables these behaviours at the Lua level:

- `a <= b` falls back to `not (b < a)` through `__lt` when neither operand has `__le`
  (`LUA_COMPAT_LT_LE`). This works across yields, as it does in C.
- `math.atan2`, `math.cosh`, `math.sinh`, `math.tanh`, `math.pow`, `math.frexp`,
  `math.ldexp` and `math.log10` are present (`LUA_COMPAT_MATHLIB`). `ldexp` casts its
  exponent to a 32-bit `int`, as C does.

`LUA_COMPAT_APIINTCASTS` and the other macros under `LUA_COMPAT_5_3` only affect the C
API, so they have no Lua-level effect.

`Limits::default().strict_5_4(true)` makes a state strict Lua 5.4, as built without the
flag. `__le` must then exist (otherwise "attempt to compare two table values"), and the
functions above are absent. The setting is fixed when the state is made: `Lua::set_limits`
keeps it (and `hash_seed`) whatever the new limits say, so `__le` and the math table always
agree.

The C Lua oracle in the tests is built with `LUA_COMPAT_5_3`. Strict mode is tested
against fixed expectations (`tests/sweep3_spec.rs`).

### Warnings

`warn` behaves as in C Lua:
- Warnings start off, as `luaL_newstate` leaves them.
- A one-piece message `"@on"` or `"@off"` switches them; other `"@..."` messages are
  ignored.
- Text goes where `print` goes, and only when the `PRINT` library is allowed.

A host turns them on or off with `Lua::set_warnings(bool)`, and a script can still switch
them afterwards.

### Testing

The test suites compare RemedyLua with C Lua 5.4.7 on every program they run. The
comparison accepts exactly one class of difference: C Lua stops with one of the
implementation-limit errors above while RemedyLua runs the program correctly
(`tests/common/mod.rs`, `C_LIMIT_ERRORS`).

The differential fuzzers (`tests/sweep1_fuzz`, `tests/sweep4_fuzz.rs`) also set aside what
cannot be compared, each on evidence of its own (`harness::classify`): `pairs` order, C Lua
disagreeing with itself (`%p`, clocks), `math.fmod`'s argument check order (C leaves it
unspecified) and C Lua's limits. Their wrapper seeds `math.random` and takes the libraries
RemedyLua leaves out of C Lua's globals, so those compare.

C Lua is always there to compare against. The `remedy-lua-oracle` crate builds it from
the Lua 5.4.7 sources named by `LUA_SRC` (`tools/gate.sh` sets it to the `lua-src`
crate's copy when it is unset). Without the sources, the build of the tests and benchmarks fails
with a message saying how to set `LUA_SRC`. A comparison is never skipped or reported
as unchecked.

Three things in the oracle differ from a stock C Lua process, all for the fuzzers' sake:
each new state takes a fresh string-hash seed (stock C Lua's seed repeats within one
process, so reruns never showed its other `pairs` orders); `remedy_lua_oracle::run`
caps C Lua's memory at 2 GiB (C Lua has no limit of its own); and its step limit counts
the instructions of every coroutine (a count hook every 1,000 instructions drawing on one
budget, starting at each new coroutine's first instruction; stock C Lua gives each
coroutine a fresh count). A run that reaches the memory cap or the step limit is the
oracle's own limit, with no verdict.

## Capabilities: what C Lua has and RemedyLua does not

RemedyLua runs other people's code, so the parts of C Lua 5.4.7's standard library that
reach the file system, the process, the environment or the interpreter's internals are
absent. Two safe pieces are there as capabilities a host grants (`Libs::DEBUG_INFO`,
`Libs::REQUIRE`): off in `Libs::SAFE`, `Libs::STANDARD` and `Libs::ALL`, so no host's
scripts gain them without the host asking.

### What is absent, and why

| C Lua 5.4.7 | Here | Why |
|---|---|---|
| `io.popen`, `io.tmpfile`; `io` on the process's files and streams | absent; `io` over the host's streams (`Libs::IO`) | `popen` runs a program, `tmpfile` makes a file. With `Libs::IO` the rest of `io` works on streams the host gives: its standard streams and the files its opener hands out. |
| `dofile`, `loadfile` | absent | They read files. `load` takes source text (`Libs::LOAD`); a host serves modules through `require`. |
| `os.execute`, `os.exit`, `os.remove`, `os.rename`, `os.tmpname` | absent | They run programs, end the host process or touch the file system. |
| `os.getenv` of the process | the host's map only (`Libs::ENV`) | The process environment is the host's (secrets, paths). `os.getenv` reads only what the host put in the state's map (`Lua::set_env`). |
| `os.setlocale` other than `"C"` | `nil` (a locale this installation lacks) | The locale is process-wide state: one script changing it would change every other state's `os.date` and number formatting in the process. Queries and `"C"` work (`Libs::OS`). |
| binary chunks: `load` of bytecode, `string.dump` | `load` refuses bytecode (`attempt to load a binary chunk`); `string.dump` raises `unable to dump given function (RemedyLua has no bytecode format)` | Bytecode is not checked: hand-made bytecode breaks any VM's invariants. |
| `debug` (all but the two functions below): `sethook`, `gethook`, `getlocal`, `setlocal`, `getupvalue`, `setupvalue`, `upvalueid`, `upvaluejoin`, `getmetatable`, `setmetatable`, `getregistry`, `getuservalue`, `setuservalue`, `setcstacklimit`, `debug.debug` | absent | They read or write other functions' locals and upvalues, every value's metatable (the string metatable, a host's userdata), the registry, or the hooks: each one is a way out of a sandbox (or, for `debug.debug`, reads the console). Even read-only, `getlocal` (by level) and `getupvalue` show a trusted caller's or a host module's private state; a hook, even metered, can raise an error at any instruction of a host module's code. |
| `debug.getinfo`'s `f` and `L` options | refused: `bad argument #2 to 'getinfo' (option 'f' is not available)` | `f` hands out the function running at a level: a script called back by a trusted caller would get the caller's closure. `L` needs the function in C Lua too. |
| `package.path`, `package.cpath`, `package.config`, `package.searchpath`, `package.loadlib`, and the Lua-file, C-library and all-in-one searchers | absent | Search paths and files, and native code. Modules come from the host. |

### `Libs::DEBUG_INFO`: `debug.traceback` and `debug.getinfo`

Both behave as in C Lua 5.4.7, results and messages, wherever the behaviour is shared
(`tests/libs_capability.rs` compares them with C Lua):

- `debug.traceback([thread,] [message [, level]])`: C's `luaL_traceback` text: the first
  10 levels and the last 11 (with `...	(skipping n levels)` between), each named as C
  names it: a global function name found in the loaded modules first (`function
  'string.rep'`, `function 'f'` for `_G.f`), then the call site's name (`local 'f'`,
  `method 'm'`, `field 'f'`, `upvalue 'f'`, `for iterator 'for iterator'`, `metamethod
  'index'`), then `main chunk`, `function <src:line>` or `?`; `(...tail calls...)` after a
  frame entered by a tail call. A message that is not a string or number is returned
  untouched; the message is cut at its first zero byte, as C's `%s` cuts it. The level is
  converted as C's `(int)` converts it. As a message handler (`xpcall(f,
  debug.traceback)`), the function that raised the error is a level, as in C Lua.
- `debug.getinfo([thread,] f-or-level [, what])`: options `S` (`source`, `short_src`,
  `what`, `linedefined`, `lastlinedefined`), `l` (`currentline`), `n` (`name`,
  `namewhat`), `u` (`nups`, `nparams`, `isvararg`), `t` (`istailcall`) and `r`
  (`ftransfer`, `ntransfer`, always 0: there are no hooks). Without `what`, the options
  are C Lua's default without `f` (`"lnSrtu"`): the table never holds a function. A level
  out of range gives `nil`; levels and messages as C Lua's (`invalid option`, `number
  expected`).
- Another thread's stack: a suspended coroutine's level 0 is `coroutine.yield`, and a
  coroutine that resumed another is at `coroutine.resume`, as in C Lua.
- A coroutine that died by an error keeps its stack, as in C Lua, so
  `debug.traceback(co, err)` after a failed `resume` shows where it failed, the
  library function that raised the error included, until `coroutine.close(co)` (or the
  collector) frees it. Frames inside a library function's callbacks (a `sort`
  comparator) are not kept, where C Lua keeps them.
- `nups` of a library function is C Lua's (3 for `string.gmatch`'s iterator, 1 for a
  `coroutine.wrap` function, `require` and the searchers, otherwise 0).

Metering: each call is charged for the levels it walks (a unit per two levels), the
bytecode it reads to name call sites, every slot of every loaded module the name search
reads (charged up front, whether or not it stops early, so fuel never follows the hash
seed), and the text it builds. Calibrated against plain code.

### `Libs::REQUIRE`: `require` and `package`, with the host's modules

`require` follows C Lua 5.4.7's `loadlib.c`: `package.loaded` (the same table C keeps in
the registry: every opened library is in it, `_G` included), `package.preload`, and
`package.searchers`, which a script may change. `require(name)` returns the module and
the loader's second value; a module whose loader returns nothing is `true`; messages are
C's (`module 'x' not found:` with each searcher's line, `'package.searchers' must be a
table`). As in C Lua there is no cycle detection: modules that require each other recurse
until the depth of nested library calls (`Limits::native_depth`, 200 as C's C levels)
runs out: the catchable error `C stack overflow`, as in C Lua.

The searchers are the preload searcher (C's, the same message) and the host's. The host
gives modules as source text, never files:

```rust
lua.add_module("greet", "return { hi = function(n) return 'hi ' .. n end }");
lua.add_module_source("cfg", ModuleSource::new(source, "mods/cfg.lua"));
lua.set_module_loader(Some(|name: &str| fetch(name).map(|src| ModuleSource::new(src, name))));
```

A host module compiles as `load` compiles text (charged the same, fuel and memory), as
chunk `@origin`, so messages and tracebacks show `origin:line:`; `require` gives `origin`
as its second result, as C gives a file name. A syntax error is `error loading module
'x' from host module 'origin':` and the message. A module not found adds `no host module
'x'` to the message. The host's own copy of a source is not counted against the memory
limit; the loader runs on the host's time and gets no access to the state. Native
modules: a host puts a table or a host function in `package.preload` (or
`package.loaded`).

## Hashing seed and replays

Each state seeds its hashing (string interning and table key placement) and
`math.random`'s first state at random when it is made, as C Lua does. This is a
security defence: a script cannot choose keys that all land in one hash slot. It also
means `pairs` order, and so the fuel a program uses, can differ from one state to the
next.

`Limits::hash_seed(u64)` fixes the seed instead. With the same seed, the same program
gives the same results, `pairs` order and fuel on every run, with the fast loop on or
off. `math.randomseed()` without arguments then takes its seed from the generator
instead of the clock. `os.time`, `os.clock` and `os.date` still read the real clock.

The fixed seed is for replays and tests. Production states should keep the default
random seed. A script that knows a fixed seed can choose keys that share hash slots;
tables bound what that costs (every key has two slots and joins the shorter of their
chains, a lookup walks at most 4 keys past each of its two slots free, further steps are
fuel, and an insert that would go past that rehashes the table under a new seed), but
memory latency is not metered: such keys spread over many tables that do not stay in the
caches cost up to about 4x (fast loop: 5x) plain code per unit of fuel, against 17x
(34x) with one slot per key. (Each link of an `__index` or `__newindex` chain is 2
units of fuel, so a chain through many tables is paid for as the loads it does.) Keep a
fixed seed secret from scripts.

The test suites use a fixed seed wherever they compare fuel or traversal order
(`tests/common/mod.rs`, `TEST_SEED`). They do not use it where they check that the
default seed varies.

## The length of a table with holes

`#t` may return any border of a table with holes (the manual's rule). RemedyLua sizes
tables exactly as C Lua 5.4.7 does (`ltable.c`: the array part, its border hint, and when
a new key makes the table rehash), so it returns C Lua's border:

- For a table whose keys outside the array part are all integers, `#t` is the same as in
  C Lua in every case, holes, deletions and `table.insert`/`table.remove` included.
- Once a key of another type (a string, say) is in the hash part, where C Lua places it
  depends on C Lua's own random hash seed, and with it, after deletions, the moment the
  table rehashes. C Lua itself can then return different borders for the same program
  from one run to the next. RemedyLua returns one of the borders C Lua can return: it
  matches C Lua's range of behaviour, not one particular run of it.

## The standard library

Every function behaves as in the Lua 5.4 manual and C Lua 5.4.7, with the same results and
the same error messages, except where this section says otherwise. What it adds is mostly
cost: library work is charged as fuel and memory, so a script cannot do unbounded work or
allocate without limit inside one call (see [Budgets](#budgets)). A charge is paid even
when the function fails.

### Libraries and capabilities

`Libs` flags choose what a state gets (`Lua::new(limits, libs)`, and `Lua::open_libs` for
more later). They combine with `|` and `-`.

| Flag | Gives |
|---|---|
| `BASE` | `assert error getmetatable setmetatable rawequal rawget rawlen rawset select tonumber tostring type pcall xpcall warn ipairs next pairs`, `_G`, `_VERSION` (`"Lua 5.4"`) |
| `STRING` | `string` and strings' metatable (`s:upper()`; arithmetic on numeric strings) |
| `TABLE` | `table` |
| `MATH` | `math` |
| `UTF8` | `utf8` |
| `COROUTINE` | `coroutine` |
| `OS` | `os.time`, `os.clock`, `os.date`, `os.difftime`, `os.setlocale` (the `"C"` locale only) |
| `LOAD` | `load` (source text only) |
| `PRINT` | `print`, and `warn`'s output |
| `GC` | `collectgarbage` |
| `SAFE` | `BASE` through `OS`: everything but `load`, `print` and `collectgarbage`. For untrusted code. |
| `STANDARD` | `SAFE` + `PRINT` + `GC` |
| `ALL` | everything above (not `DEBUG_INFO`, `REQUIRE`, `ENV` or `IO`) |
| `DEBUG_INFO` | opt-in: `debug.traceback`, a read-only `debug.getinfo` ([capabilities](#capabilities-what-c-lua-has-and-remedylua-does-not)) |
| `REQUIRE` | opt-in: `require` and `package`, with modules only from the host |
| `ENV` | opt-in: `os.getenv` from the host's map (`Lua::set_env`) |
| `IO` | opt-in: `io` over the host's streams and files ([`io`](#io-libsio)) |
| `NONE` | nothing |

`LOAD`, `PRINT` and `GC` take effect with `BASE`.

Absent by design, because the state is a sandbox: `io.popen`, `io.tmpfile`, `dofile`,
`loadfile`, the rest of `debug` and `package`, and `os.execute`, `os.exit`, `os.remove`,
`os.rename`, `os.tmpname`; `os.getenv` reads only the host's map (`Libs::ENV`) and
`os.setlocale` knows only `"C"`. There are no binary chunks: `load` refuses
them, and `string.dump` errors. `debug.traceback`, `debug.getinfo` and `require` are
opt-in capabilities, as is `io` over the host's streams (`Libs::IO`); each absent piece and its reason are in
[capabilities](#capabilities-what-c-lua-has-and-remedylua-does-not). A host gives a script what it needs through host functions
(`Lua::register`).

### Base functions

- **`error`**: a string message gets its position as in C. The message is cut to
  `Limits::string_len`.
- **`setmetatable`**: a metatable with a `__gc` field is refused, with "__gc metamethods
  are not supported in RemedyLua". No `__gc` ever runs, even if added later; a host that
  needs to know when a value is collected uses tracked userdata
  ([below](#userdata-tracked-userdata-and-app-data)).
- **`next`, `pairs`**: the order is C Lua's kind of order, from the state's hash seed
  ([Hashing seed](#hashing-seed-and-replays)).
- **`select('#', ...)`, `select(n, ...)`**: a huge `n` gives nothing, as in C.
- **`tonumber`, `tostring`**: as in C. `tostring` of a table, function, userdata or
  thread writes `type: 0x...` (or the `__name`, cut to 256 bytes). The number is not a
  memory address: it depends only on which object it is. `__tostring` must return a string
  or a number ("'__tostring' must return a string").
- **`pcall`, `xpcall`**: as in C, and yieldable. A limit (`Error::Limit`: fuel, memory,
  interrupt, time) is not an error a script can catch: `pcall` and `xpcall` pass it on. An
  `xpcall` handler that keeps failing stops with "error in error handling", as in C.
- **`print`**: writes each argument as `tostring` does, tab-separated, then a newline, to
  the host's buffer or sink ([Output](#output)), never to stdout. With `Libs::PRINT` only.
- **`warn`**: as in C (off at first; `"@on"` and `"@off"`), written where `print` writes,
  as `Lua warning: ...` lines. It is in `BASE`, and writes only with `Libs::PRINT`
  ([Warnings](#warnings)).
- **`load(chunk [, name [, mode [, env]]])`** (with `Libs::LOAD`):
  - Text only. A binary chunk (starting with byte 27) fails with "attempt to load a binary
    chunk (bytecode is not accepted)". With a `mode` without `t`, load returns nil and
    "attempt to load a text chunk (mode is '...')".
  - A `name` ending in `.rlua` compiles the RemedyLua dialect.
  - A reader function is called as the lexer needs more text, as in C: as often as C Lua
    calls it, and not again once a syntax error is found. An error the compiler finds
    (an assignment to a `<const>`, a `goto` with no visible label, a `break` outside a
    loop) is found once the text has been read, so the reader is called to its end there,
    where C Lua stops at the error; the message is C Lua's. Its
    pieces are kept up to `Limits::string_len` (past it, the memory limit). The reader
    cannot yield.
  - Compiling is charged as fuel: the source's length plus 6 per token plus 100, and the
    compiler's lookups. A syntax error is charged again and returned as `nil, message`, in
    C Lua's form (`chunk:line: message`). Compiling is bounded by memory, not by C's
    nesting limits.
  - A fourth argument is the environment, even when nil (as in C).
- **`collectgarbage`** (with `Libs::GC`): `"collect"` (the default) runs a full
  collection; `"count"` is the state's counted memory in KiB (`Lua::memory_used`, what the
  memory limit counts, not the process's); `"step"` (or `n` = 0) runs one step of the
  collector's own size (at least 8 KiB of work; C's basic step is bigger) and returns true
  if the cycle ended or began in it; `"step", n` with `n` > 0 is C's: `n` KiB more debt,
  then steps until that debt and one step size (8 KiB) are worked off, in C's work units
  (single steps of the usual size, each charged as it ends), or the cycle ends, returning
  true only when it ended; a negative `n` does no step; `"isrunning"`, `"stop"` and `"restart"` are as in C;
  `"incremental"` and `"generational"` return the previous mode's name. There is one
  (incremental) collector, so `"generational"` only changes the name returned. `"setpause"`
  and `"setstepmul"` return the previous value, and keep values in 0..1023 (rounded down to
  a multiple of 4). The collector's work is charged as fuel.

### `string`

- **Results bounded by `Limits::string_len`** (16 MiB by default). `string.rep`,
  `string.pack` and `..` past it: the memory limit (`Error::Limit(Memory)`), except
  `string.rep` and `string.pack` with a result over the limit known up front, which raise
  "resulting string too large", as C does for its own limit. `string.format`, `gsub`,
  `table.concat` and `os.date` past it: the memory limit. Memory is charged before a result
  is built.
- **Patterns** (`find`, `match`, `gmatch`, `gsub`): C Lua's patterns, with C's limits (32
  captures, "too many captures"; 200 levels of backtracking, "pattern too complex"). The
  matcher does not recurse on the native stack. One match may take at most 4,000,000 + 64
  per subject byte steps, and no more than the fuel left allows. Past that it raises
  "pattern too complex" (an ordinary error, which `pcall` catches), where C Lua would keep
  going for as long as the pattern takes. Each 4 steps cost 1 fuel. The host's interrupt
  and deadline are read during a match.
- **`gsub`** with a function or table replacement calls it as a callback: it cannot yield,
  and counts against `Limits::native_depth`.
- **`format`**: as in C, including `%q` (floats in hexadecimal, `math.mininteger` as
  `0x8000000000000000`, infinities as `1e9999`, NaN as `(0/0)`) and `%a`. `%p` writes
  `(null)` for values without an identity and a stable `0x...` for objects (and strings).
- **`byte`**, **`utf8.codepoint`**, **`table.unpack`**: their results are limited by
  memory, not by C's stack size (`Limits::stack_slots` is unlimited by default). The memory
  for the results is charged before they are made.
- **`dump`**: always fails: "unable to dump given function (RemedyLua has no bytecode
  format)".
- Charges: about 1 fuel per 32 to 64 bytes made or scanned (`sub`, `upper`, `lower`,
  `reverse`, `rep`, `format`, plain `find`, `gsub`'s output, `tonumber` of a long string),
  1 per 8 results (`byte`, `char`, `unpack`, `select`), and 8 per float written as text.

### `table`

- `insert`, `remove`, `move`, `concat`, `unpack`, `pack`, `sort` as in C, honouring
  `__index`, `__newindex` and `__len`. Each element moved, joined, unpacked or compared
  costs 1 fuel.
- **`sort`**: the comparator is called as a callback (it cannot yield). "invalid order
  function for sorting" comes as in C. Where C Lua picks a random pivot from the clock (for
  big, badly split arrays), RemedyLua uses a generator seeded from the state, so a fixed
  hash seed replays sorts exactly.
- **`concat`**: a value that is not a string or number is "invalid value (TYPE) at index k
  in table for 'concat'". The result is bounded by `string_len`.

### `math`

- As in C. With `LUA_COMPAT_5_3` behaviour (the default), also `atan2`, `cosh`, `sinh`,
  `tanh`, `pow`, `frexp`, `ldexp` and `log10`; none of these with `Limits::strict_5_4`
  ([Lua 5.3 compatibility](#lua-53-compatibility-on-by-default-and-strict-54)).
- **`random`**: xoshiro256\*\*, as C's, seeded per state at random (one way from the hash
  seed, so a script's random numbers do not reveal it). With `Limits::hash_seed` fixed,
  `math.randomseed()` without arguments seeds from the generator, so runs replay.

### `utf8`

As in C (`char`, `charpattern`, `codepoint`, `len`, `offset`, `codes`, with the `lax`
flags). `codepoint`'s results are bounded by memory; work is charged per 16 bytes walked.

### `coroutine`

- As in C: `create`, `resume`, `yield`, `status`, `wrap`, `isyieldable`, `running`,
  `close`. Coroutines run inside the interpreter loop, so a coroutine can yield from inside
  `pcall` and from metamethods written in Lua.
- A coroutine cannot yield across a callback that a library or the host runs:
  `table.sort`'s comparator, `gsub`'s replacement function, `__tostring` (in `tostring`
  and `print`), `load`'s reader, `xpcall`'s handler, `coroutine.close`'s pending `__close`,
  and a host function or what it calls back into. That is "attempt to yield across a C-call
  boundary", as for C functions in C Lua.
- Every step a coroutine runs counts against the fuel of the host call it runs inside.
  A resume is refused where C Lua's is: when the resuming thread already holds C Lua's 200
  C levels, counted as C Lua counts them (each resume, protected call and metamethod call
  on the chain of resumers, and a `pcall` of the resuming function), so `pcall(co)` nested
  stops at 99 levels and plain `coroutine.resume` chains at 198. At C Lua's 199th level the
  resume runs and a coroutine not yet started fails as its body is called, before any of
  it runs, so it is dead afterwards (a suspended one runs on); at the 200th the resume is
  refused and the coroutine stays as it was. The message is "C stack overflow" with no
  position either way: `coroutine.resume` returns it, `coroutine.wrap` raises it
  (positioned at its caller when that is a Lua function, as C Lua's). Resumes nested deeper
  than `Limits::native_depth` are refused the same way.

### `os`

- **`os.time`**, **`os.date`**, **`os.clock`** read the clocks the host gives
  (`Lua::set_os_clocks(wall, cpu)`). By default on native targets, `wall` is the system
  clock and `cpu` is the time since the state was made. In a browser there are none, and
  they read 0. Local time is UTC plus the host's offset (`Lua::set_utc_offset`, 0 by
  default). There is no daylight saving time, so `isdst` is always false and `%Z` is
  `UTC` or empty.
- **`os.time(t)`** reads the table's fields through `__index` and writes the normalized
  ones back through `__newindex` (`year month day hour min sec yday wday isdst`, in C's
  order), as C does. `isdst = true` gives an hour less, as glibc's `mktime` does in a zone
  without daylight saving time; the field is written back as false. `os.date` supports C99's
  conversions and their `E`/`O` modifiers; an invalid one is an error, as in C.
- **`os.difftime(t2, t1)`** takes two integers.
- **`os.getenv(name)`** (`Libs::ENV`) returns what the host set for `name` with
  `Lua::set_env(name, Some(value))`, or nil; never the process environment, which nothing
  reads. The name ends at its first zero byte, as C's `getenv` reads it. Charged 4 fuel plus
  1 per 32 bytes of name and of value; a name longer than any the host set is not hashed.
  The value's copy in the state counts against the memory limit; the host's map does not.
- **`os.setlocale([locale [, category]])`** answers as C Lua does in a process that stays
  in the `"C"` locale: a query (no locale, or `""`) and `"C"` or `"POSIX"` give `"C"`;
  any other locale gives nil, as for one the system lacks. The category is checked as in C
  (`invalid option`). The process locale is never changed.

### `io` (`Libs::IO`)

C Lua 5.4.7's `io` (`liolib.c`), with the same results and messages, over streams the host
gives instead of the process's files:

- **The standard streams** are the host's: `Lua::set_stdin`, `set_stdout` and
  `set_stderr` take a `Box<dyn Stream>`. Without one, stdin is empty (reads see its end)
  and stdout and stderr write where `print` writes (the host's sink or `take_output`), in
  order with it, within `Limits::output`. They are `io.stdin`, `io.stdout`, `io.stderr`,
  the default input and output, and cannot be closed ("cannot close standard file").
- **Files** come from the host's opener (`Lua::set_file_opener(|name, mode| ...)`), which
  `io.open`, `io.lines(name)`, `io.input(name)` and `io.output(name)` ask with the name (up
  to its first zero byte) and a mode `io.open` accepted (C's `[rwa]%+?b*`, else "invalid
  mode"). Its `IoError { message, code }` is the script's `nil, "name: message", code`, or
  "cannot open file 'name' (message)" where C raises. Without an opener every name is "No
  such file or directory" (2). The files are the host's: what a script writes to them is
  the host's memory, not the state's, so a host bounds it (a full disk is `IoError::new("No
  space left on device", 28)`).
- **A `Stream`** implements `read`, `write`, `flush`, `seek` and `close`; each defaults to
  C's failure (`Bad file descriptor` for reading or writing, `Illegal seek`). The calls run
  inside the script's call: a stream that blocks holds the script (fuel and the deadline
  are not read meanwhile).
- **Reading**: `read` with `n`, `l`, `L`, `a` and counts (and the `*` prefix), as C reads
  them: `n` reads C's longest numeral prefix (at most 200 bytes) and converts it as
  `tonumber` does. A file reads ahead 4 KiB, given back (the stream seeks back) before a
  write or a seek. `read(k)` does not allocate `k` bytes up front as C does: a huge count
  reads what there is (C fails for lack of memory).
- **Writing**: numbers as `%d` and `%.14g`, as C writes them.
- `lines` (both), `seek`, `setvbuf` (accepted, nothing changes), `flush`, `close`,
  `io.type`, `__close`, `__tostring` (`file (0x...)`, `file (closed)`), `__name` `FILE*`:
  as in C. **No `__gc`**: a file a script drops unclosed is dropped (its `Stream` with it)
  when collected, never closed.
- **Metered**: each call 4 fuel, and 1 per 32 bytes read or written; a read's result is
  bounded by `Limits::string_len` and memory (past them, the memory limit); each file
  handle counts its 4 KiB of read-ahead as memory. An endless stream read whole stops on
  the fuel or memory limit.

### Metamethods

All of Lua 5.4's metamethods work as in C Lua (`__index`, `__newindex`, `__call`, the
arithmetic, bitwise and comparison events, `__concat`, `__len`, `__unm`, `__close`,
`__mode`, `__name`, `__metatable`, `__tostring`, `__pairs`), with one exception: **`__gc`
is not supported** (it is refused by `setmetatable` and `Lua::set_metatable`, and never
runs). Metamethods written in Lua run in the interpreter loop: they can yield, and cost
fuel like other code. `__index` and `__newindex` chains longer than 2,000 raise "'__index'
chain too long; possible loop", as in C. A `__mode` is read from a short string (40 bytes
or less).

## Budgets

Every bound a script can reach is in `Limits`, and every one is exact: it holds the same
with the fast loop on and off, and whatever the hash seed.

| `Limits` field | Default | What it bounds | Past it |
|---|---|---|---|
| `fuel` | 10,000,000 | Steps per call from the host: one per instruction, plus library work and the collector's work | `Error::Limit(Fuel)` |
| `memory` | 64 MiB | Bytes of the state's objects, counted before they are made, and the free slots their kinds keep (a collected object's slot, until one of its kind reuses it or none sits past it) | `Error::Limit(Memory)` |
| `string_len` | 16 MiB | The longest string | `Error::Limit(Memory)` (`string.rep` and `string.pack` known up front: "resulting string too large") |
| `call_depth` | unlimited (bounded by `memory`) | Lua frames per thread | "stack overflow", an ordinary error `pcall` can catch, as at C Lua's stack limit |
| `native_depth` | 200 | Callbacks nested inside library functions and the host (`sort` comparators, `__tostring`, host functions calling back) | "stack overflow (too many nested calls from library functions)"; for nested resumes, "C stack overflow" |
| `stack_bytes` | 512 KiB | The native stack one call may use for those callbacks | as `native_depth` |
| `stack_slots` | unlimited | Value-stack slots per thread (16 bytes of memory each) | "stack overflow" |
| `output` | 1 MiB | Bytes `print` and `warn` write per call from the host | the rest is dropped; the buffer `take_output` drains is charged to `memory` until taken |
| `error_len` | 4 KiB | The longest error message kept (and names in messages) | cut |
| `hash_seed` | random | The seed of hashing and `math.random` | (see [Hashing seed](#hashing-seed-and-replays)) |
| `strict_5_4` | false | Lua 5.3 compatibility off | (see [compatibility](#lua-53-compatibility-on-by-default-and-strict-54)) |

`Limits::unmetered()` lifts fuel, memory, `call_depth`, `string_len`, `output` and
`error_len`. It keeps `native_depth` and `stack_bytes`, which protect the native stack.
Start from `Limits::default()` or `unmetered()` and use the setters: the struct is
`#[non_exhaustive]`.

What costs fuel besides instructions: library work (per byte, element, result or
comparison, as in [the library](#the-standard-library)), compiling (`load`), the
collector (1 fuel per 4 units of its work), argument and result lists past 16 values,
probing tables' hash chains, each link of an `__index` or `__newindex` chain (2 fuel,
the string library's methods included), and making error messages. The host's interrupt and time
limit are read between fuel slices (16,384 steps), wherever a library function charges,
and while compiling, so a call stops within a fraction of a millisecond of work after
either ([Interrupts and time limits](#interrupts-and-time-limits)).

Fuel follows time, measured against plain code. When an operation gets faster, plain code
costs less time per unit of fuel, and the operations whose fuel was set against the slower
plain code are charged more. They are never let off. Two charges rose this way:
- the walk over the threads that resumed one, when an `xpcall` handler runs inside a
  coroutine: a unit per 5 frames (was 16);
- `string.pack`, `string.packsize` and `string.unpack`: 2 units per format byte (was 1 per
  8).

Other programs use the same fuel as before.

A table's rehash is charged by what it does: its passes and each key it places again. It
was charged after it was done, so a call could run past its fuel by one rehash of the
biggest table it held. Now the least a rehash costs is worked out from the table's sizes
first, and a call that cannot pay for it stops at its fuel limit before the table changes.

Memory is counted per object: strings (64 bytes plus their length, and 16 more past the
22 bytes a string keeps inline: what one holds resident; strings of 40 bytes or less are
interned), tables (their parts), functions, upvalues, userdata (their declared
size), threads, and value-stack slots. Up to 64 KiB of freed table buffers are kept for
reuse without being counted. An allocation that finds memory full first runs a full
collection and tries again once.

**Known limitation: a stall at the memory limit.** The limit is hard: an allocation goes in
only when its charge fits. When it does not, the allocation runs a full collection, which
finishes the cycle under way and then runs a whole new one, charged to the call as it goes.
When less than a quarter of the cap remains and sweeping the table slab alone would exceed
a call's fuel, the collector shortens the allocation interval between steps. It traces new
Lua table constructors through their owners, allowing unreachable tables to be reclaimed
in the current cycle. On the 800,000-key workload at 512 MiB and 20,000 fuel per call,
the first 8,000 calls needed no emergency collection. Incremental collection can still
occupy consecutive calls, and other workloads can still reach the hard cap. A host that cannot afford such a run of calls should give
its state more memory headroom or larger calls. `Lua::collector_status` shows where the
collector is: its phase, the fuel already owed (`owed_fuel`, exact, charged first by the
collector's next steps), and a floor on what the cycle under way still needs
(`remaining_fuel_at_least`, with `more_may_be_found` when the rest is not known ahead, as
while marking).

**Known limitation: a rehash the host keeps undoing.** A table rehash bigger than a call's
fuel is made across calls: each call pays for work that is kept, and the new key goes in
once the whole rehash is paid. A host write that changes the table's layout between those
calls (for example `T[k] = nil` then `T[k] = v` from Rust) discards the kept work and the
fuel paid for it, and the next call starts the rehash again. A host that writes to the
same table more often than the rehash takes to finish (measured: a 2^22-slot array part at
20,000 fuel a call needs 132 calls; writes every 20 or 60 calls kept it from ever finishing,
writes every 200 did not) keeps the script's write from going in. The script's own fuel is
charged only for work it did. A host should leave a table alone while a script's call on it
is stopped mid-rehash, or give such calls more fuel.

The front end has no fixed nesting limit: the parser, compiler, syntax-tree drop, `check`
and `format` use explicit stacks charged to memory. For `load` in a state, the state's
memory and fuel bound them. `check` and `format` have no state, so they may use 1 KiB per
source byte plus 64 MiB ("not enough memory" past it), and the checker as much again for its
own tables.

## Embedding guide

This is for engines that run untrusted scripts inside an application: a UI's widgets, a
chat app's bots, a game's mods. The rustdoc of `remedy_lua` has every item; this section
is how they fit together.

### A state and its capabilities

```rust
use remedy_lua::{Lua, Limits, Libs, Value};
let mut lua = Lua::new(Limits::default().fuel(1_000_000).memory(4 << 20), Libs::SAFE);
lua.register("host.greet", |lua, args| {
    let who = args.first().and_then(|v| lua.to_string_lossy(*v)).unwrap_or_default();
    Ok(vec![lua.create_string(format!("hi {who}"))?])
})?;
let out = lua.exec("return host.greet('you')", "=widget")?;
```

- `Libs` chooses the libraries: `SAFE` for untrusted code (no `load`, `print` or
  `collectgarbage`), `STANDARD` adds `print` and `collectgarbage`, `ALL` adds `load`
  (source text only). `open_libs` grants more later. Two capabilities are in none of
  these presets, so a host gets them only by naming them: `Libs::DEBUG_INFO`
  (`debug.traceback` and a read-only `debug.getinfo`) and `Libs::REQUIRE` (`require` and
  `package`, with modules only from the host). There is no `io`, the rest of `debug` or
  `package`, and `os` has only `time`, `clock`, `date` and `difftime` (see
  [the standard library](#the-standard-library) and
  [capabilities](#capabilities-what-c-lua-has-and-remedylua-does-not)).
- `Limits` is `#[non_exhaustive]`: start from `Limits::default()` (or `unmetered()`) and
  use its setters (`fuel`, `memory`, `call_depth`, `native_depth`, `stack_slots`,
  `string_len`, `output`, `error_len`, `stack_bytes`, `hash_seed`, `strict_5_4`).
- `set_limits` changes them between calls (fuel applies from the next call from the
  host). `hash_seed` and `strict_5_4` are read only when the state is made; `set_limits`
  keeps the state's own.
- A `Lua` is `Send`, not `Sync`: one thread uses it at a time, and it may move between
  threads. `Interrupt`, `Error`, `Value`, `Root` and `Data` are `Send + Sync`.

### Calls, fuel and limits

Each call from the host (`exec`, `call`, `call_global`, and the smaller entries such as
`create_table` or `get`) starts with the whole fuel budget. A call made from inside a host
function shares the budget of the call it is in. A call that runs past a limit returns
`Error::Limit(kind)`, and no `pcall`, `xpcall`, coroutine or `__close` can catch it. The
state stays usable.

`LimitKind` is `#[non_exhaustive]`: match it with a `_` arm. Its kinds are:

- `Fuel`: one step per instruction, plus the work of library functions and the collector.
- `Memory`: the memory cap, or a string longer than `string_len`.
- `Interrupted`: the host's interrupt.
- `Time`: the time limit.

`fuel_left()` after a call tells what it used. `memory_used()` is the state's heap now.

The memory limit is on what the state counts (`memory_used()`): its objects, the library's
buffers while they are held, and the compiler's tokens, tree and code. That count never
exceeds the limit. The process's resident memory can exceed it by what the allocator holds
besides: freed blocks it keeps, and fragmentation (glibc's, with large strings mixed among a
parse's small blocks, was measured at up to about 8 MiB over a 64 MiB limit).

### Interrupts and time limits

- **Interrupts.** `Lua::interrupt_handle()` returns an `Interrupt`, a cheap clonable
  handle to the state's one interrupt flag. Setting it from any thread with `interrupt()`
  stops the running call with `Error::Limit(LimitKind::Interrupted)`. The stop comes within
  a fraction of a millisecond of work, wherever the call is: the interpreter loop, a library
  function such as `gsub` or `sort`, the compiler, or a coroutine. Scripts cannot catch it.
  It stays set, and every entry into the state stops at once, until `reset()`. A host that
  reuses a state resets the handle before the next call it wants to run.
- **Time limits.** `Lua::set_time_limit(Some((clock, limit)))` stops each call from the
  host once it has run `limit` by `clock`, with `LimitKind::Time`. The clock is a callback
  the host supplies: a wall clock, or a thread's CPU clock. A `load` from the host outside
  a call has no deadline, only the interrupt.
- **How both work.** Fuel is handed out in slices of 16,384 steps. The interrupt and the
  deadline are checked between slices, whenever a library function charges fuel, and when
  a host function returns. So a script that spends its time waiting in host functions
  stops too, without the host checking for itself. The fuel budget stays exact: a call
  still stops at the very step its fuel runs out.

### Handles and their lifetimes

A `Value` is `Copy` and does not keep what it names alive. A string, table, function,
userdata or thread the host holds is valid while one of these holds:

- it is reachable from the script (a global, a table the script keeps);
- a `Root` holds it (`Lua::root`; dropping the `Root` lets it go);
- the host made or got it outside a call (`create_string`, `create_table`,
  `create_function`, `create_userdata`, `from_data`, `get`, and the results of `call` or
  `exec`). It is then valid until the host's next `call` (or `exec`, `call_global`)
  returns, or until it calls `collect_garbage`. So the usual set-up sequence (make a table, make a key and a value, store them)
  needs no roots. One exception: when memory is full outside a call even after a full
  collection, these values are let go, so a host that never calls cannot fill memory with
  them;
- inside a host function: it is one of its arguments, or the function made it. It is
  valid until the function returns. The results of a `call` made inside a host function
  are valid until that function's next `call` (whatever else it does in between: `get`,
  `set`, making values), so a callback loop does not pile up results.

Root what must live longer: a module's table of handlers, a callback the host keeps.
`load` already returns a `Root`.

A stale handle never reaches another object. The API refuses it with `Error::Host("a
value whose object was collected")`, and the read-only accessors (`table_get`,
`table_len`, `table_pairs`, `str_bytes`, `userdata`) return nil or nothing. A handle from
another state is refused the same way.

`Value`'s `==` compares strings by identity: short strings (40 bytes or less) are
interned, long ones are not. Compare script values with `Lua::raw_equal`, which is Lua's
`rawequal`.

### Host functions

`create_function(name, f)` and `register("lib.name", f)` take an
`Fn(&mut Lua, &[Value]) -> Result<Vec<Value>, Error> + Send + Sync`.

- **Errors.** An `Err` is an error the script sees, at the call's position, and `pcall`
  can catch it, unless it is an `Error::Limit`. When the function re-raises an
  `Error::Runtime` that a `lua.call` inside it returned, the script gets its own error
  value back (a table stays a table). Use `Error::runtime("message")` for your own.
- **Reentrancy.** A host function may call back into the state (`lua.call`, `lua.exec`,
  `lua.collect_garbage`), and a script may reach it again while it runs. That is why
  `create_function` takes `Fn + Send + Sync`: the state shares the function between nested
  runs, and the state is `Send`. For mutable state, use `create_function_mut` (and
  `register_mut`). It takes an `FnMut + Send`, which need not be `Sync`, and a script
  reaching it again while it runs gets an ordinary error. Or keep the state in app data.
  A coroutine cannot yield across a host function: that is an ordinary error.
- **Panics.** A panic in a host function, a clock or a print sink unwinds out of the call
  to the host and poisons the state: every later call returns `Error::Host` (as do
  `set_metatable` and `to_data`; the plain accessors such as `table_get` only read). This
  holds even when another host function further out catches the panic and returns
  normally: the script it returns to stops. To contain a host bug, catch it inside the
  function (`std::panic::catch_unwind`) and return an error. (The time limit's clock is
  also read once as a call from the host starts, before anything runs: a panic there
  reaches the host without poisoning the state.)
- `register("a.b", f)` makes the table `a` when the global `a` is missing or not a table
  (replacing it). Both steps are raw, so a script's metatable on `_G` does not run.

### Userdata, tracked userdata and app data

- `create_userdata(data, meta)` makes a full userdata holding any `T: Any + Send`. Methods
  go in the metatable's `__index`. A `__gc` field never runs. The userdata counts
  `size_of::<T>()` plus a small header against the memory cap. What `T` owns on the heap
  (a `Vec`'s buffer) is counted once the host declares it with
  `set_userdata_size(v, size_of::<T>() + buffer.capacity())`. Declare before allocating.
  The difference is charged at once, so a script that asks a host function for big
  buffers stops at its memory limit (`Error::Limit(Memory)`, uncatchable). A size past
  `isize::MAX` is refused the same way, whatever the cap. The declared size is given back
  when the userdata is collected. The value's `Drop` runs when the collector frees it,
  inside a call: a panic there poisons the state like a host function's.
  `userdata::<T>(v)` and `userdata_mut::<T>(v)` read it back (`None` for another type or a
  stale handle).
- `create_userdata_tracked(data, meta, key)` also queues `key` for `take_collected()` once
  the collector frees the userdata. No Lua code runs at collection (there is no `__gc`).
  The host drains the queue when it chooses. Each key counts 8 bytes against the memory
  cap from when its userdata is made until `take_collected` returns it, so a script that
  makes and drops tracked userdata stops at its memory limit; drain the queue after each
  call.
- App data (`set_app_data`, `app_data::<T>()`, `app_data_mut::<T>()`,
  `take_app_data::<T>()`) keeps the host's own state with the Lua state, one value per
  type, for host functions to reach. Values must be `Send`, because a `Lua` is `Send`.

### Data in and out

`to_data(v, DataLimits)` copies a value out as owned `Data`, raw: no metamethod runs, so
reading what a script returned costs the host no script time. Functions, userdata,
threads, and tables deeper than the limit (a table that holds itself, say) are an
`Error::Host`. The node and byte limits bound what one copy may make, even when tables
share parts. `from_data(&Data)` makes a value. Nil and NaN keys are skipped, and it nests
at most 64 deep. `from_data_with(&Data, DataLimits)` checks depth (64 at most, whatever the
limit says), values and string bytes before it makes anything. `Data` is
`#[non_exhaustive]`: match it with a `_` arm.

### Output

`print` and `warn` (with `Libs::PRINT`) write to a buffer that `take_output()` drains, or
to the sink given to `set_print` (`clear_print` goes back to the buffer). At most `Limits::output` bytes per call from the host
are written; the rest is dropped. The buffer counts against the memory budget until
`take_output()` takes it, so a host that lifts `Limits::output` without a sink and never
takes the output stops the script on the memory limit, not by growing without bound.
Warnings start off; `set_warnings(true)` turns them on.

### Diagnostics

`remedy_lua::check(source, name)` parses without running. A syntax error is the only
diagnostic it returns (the first one). Otherwise, for a `.rlua` file, it returns the
[type checker](#the-type-checker)'s diagnostics: errors in source order, at most 500, and a
warning if the check stopped at its bound. A `.lua` file has no annotations, so it gets
none. Compile errors that only the compiler finds (a `goto` without its label, `continue`
outside a loop, assigning a `<const>`) come from `load`, not from `check`.

A `Diagnostic` has:

- `severity`: `Severity::Error` or `Severity::Warning`. It is `#[non_exhaustive]`.
- `chunk`, and the range `pos` .. `end` (line and column from 1; `end` is just past the
  text) for an editor.
- `line`, and `lua_message()`, which gives C Lua's own message (`chunk:line: message`).
- `message`.

`Display` writes `chunk:line:col: message` (and `warning: ` for warnings).
`remedy_lua::format(source, name)` re-indents a source. `cargo run -p remedy-lua --example
check -- file.rlua` prints a file's diagnostics.

### Weak tables

Weak tables behave as in C Lua: weak keys, weak values, both, and ephemerons. Inside a
library function, the registers its Lua caller left above the call are dead, as in C Lua,
so a value only they hold is collected.

### Building for speed

Profile-guided optimization (rustc's `-Cprofile-generate` and `-Cprofile-use`) can make the
interpreter faster on the workload it was trained on, and only on that: trained on other
scripts it gained nothing. Train it on your own scripts.
Fuel, results and memory are the same in every build.

### In a browser (WebAssembly)

RemedyLua builds for `wasm32-unknown-unknown` and needs no imports. Making a state reads
no clock. Without `Limits::hash_seed`, the seed comes from std's `RandomState`, which
works there but has no real entropy source. For hash-flooding defence in a browser, pass a
seed of your own from `crypto.getRandomValues`. Nothing else touches the OS:

- `os.time`, `os.date` and `os.clock` read the clocks `Lua::set_os_clocks(wall, cpu)`
  gives. By default these are the system's on native targets; in a browser there are none,
  and they read 0. Give `wall` from `Date.now()` and `cpu` from `performance.now()` through
  your bindings.
- Time limits use the host's `Clock` (`set_time_limit`), as everywhere.
- The interrupt is an atomic flag. Without threads, a host sets it from a host function
  or between calls.

`tools/wasm-smoke` proves it: `node tools/wasm-smoke/smoke.mjs` builds a tiny cdylib for
`wasm32-unknown-unknown` and runs a script under Node with an empty import object. The
script covers tables, sort, coroutines, `pcall`, patterns, `os`, the fuel limit and a
host clock.

## API index

Everything public in `remedy_lua`. The rustdoc has each item's details.

- **Types**: `Lua`, `Limits`, `Libs`, `LimitKind`, `Error`, `Value` (and the handle
  types `StrRef`, `TableRef`, `FuncRef`, `UdRef`, `ThreadRef`), `Root`, `Data`,
  `DataLimits` (and `MAX_DATA_DEPTH`), `Interrupt`, `Clock`, `Dialect`, `Diagnostic`,
  `Pos`, `Severity`, `ModuleSource`, `ModuleLoader`.
- **Free functions**: `check(source, name)`, `format(source, name)`.
- **A state**: `Lua::new(limits, libs)`, `open_libs`, `limits`, `set_limits`,
  `fuel_left`, `memory_used`, `collect_garbage`.
- **Running**: `load(source, name) -> Root`, `load_with(source, name, dialect)`,
  `exec(source, name)`, `call(f, args)`, `call_global(path, args)`.
- **Values**: `globals`, `get_global`, `set_global` (raw), `get` and `set` (with
  metamethods), `table_get`, `table_set`, `table_len`, `table_pairs` (raw),
  `create_table`, `create_string`, `str_bytes`, `to_string_lossy`, `raw_equal`,
  `set_metatable`, `root`.
- **Host functions**: `create_function`, `create_function_mut`, `register`,
  `register_mut`.
- **Modules** (for `require`, with `Libs::REQUIRE`): `add_module`, `add_module_source`,
  `set_module_loader`.
- **Userdata**: `create_userdata`, `create_userdata_tracked`, `set_userdata_size`,
  `userdata`, `userdata_mut`, `take_collected`.
- **App data**: `set_app_data`, `app_data`, `app_data_mut`, `take_app_data`.
- **Data**: `to_data(v, DataLimits)`, `from_data`, `from_data_with`.
- **Output**: `take_output`, `set_print`, `clear_print`, `set_warnings`.
- **Time and stopping**: `interrupt_handle` (`Interrupt::interrupt`, `reset`, `is_set`),
  `set_time_limit`, `set_os_clocks`, `set_utc_offset`.
- **Environment** (for `os.getenv`, with `Libs::ENV`): `set_env`, `clear_env`.
- **Streams and files** (for `io`, with `Libs::IO`): `set_stdin`, `set_stdout`,
  `set_stderr`, `set_file_opener`; `Stream`, `IoError`.
- **Errors**: `Error::Syntax(Diagnostic)`, `Error::Runtime { message, traceback }`,
  `Error::Limit(LimitKind)`, `Error::Host(String)`; `Error::runtime(msg)`;
  `Error::message()`.
