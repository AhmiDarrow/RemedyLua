//! Exports timed from JS (there is no clock in wasm's `std`): `make_vec`/`grow_vec`, a
//! buffer of `mib` MiB reserved to twice its size (1 if it moved: copied);
//! `make_table`/`grow_table`, an array part of 2^`bits` slots grown by one key, stopped
//! every `every` reads of a counting clock (0: never) and called again until done (one
//! call of `grow_table` a slice); and `rw`, a loop reading and writing slots `lo..hi` of
//! that table `reps` times (the steady state of a segmented array part).

use remedy_lua::{Limits, Libs, Lua, Value};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

static BUF: Mutex<Vec<u8>> = Mutex::new(Vec::new());
static STATE: Mutex<Option<Lua>> = Mutex::new(None);
static READS: AtomicUsize = AtomicUsize::new(0);
static EVERY: AtomicUsize = AtomicUsize::new(0);

#[unsafe(no_mangle)]
pub extern "C" fn make_vec(mib: u32) {
    let mut b = BUF.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    *b = vec![1u8; (mib as usize) << 20];
}

#[unsafe(no_mangle)]
pub extern "C" fn grow_vec() -> i32 {
    let mut b = BUF.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let at = b.as_ptr() as usize;
    let more = b.len();
    b.reserve_exact(more);
    i32::from(b.as_ptr() as usize != at)
}

#[unsafe(no_mangle)]
pub extern "C" fn make_table(bits: u32, every: u32) -> i32 {
    let mut lua = Lua::new(Limits::default().memory(1 << 30).fuel(1 << 40), Libs::ALL);
    if lua.exec(&format!("T = {{}} for i = 1, 1 << {bits} do T[i] = i end"), "=setup").is_err() {
        return -1;
    }
    EVERY.store(every as usize, Relaxed);
    lua.set_time_limit(Some((
        std::sync::Arc::new(|| {
            let e = EVERY.load(Relaxed);
            std::time::Duration::from_secs(if e != 0 && READS.fetch_add(1, Relaxed) + 1 >= e { 7200 } else { 0 })
        }),
        std::time::Duration::from_secs(3600),
    )));
    *STATE.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(lua);
    1
}

/// One slice: 1 when the growth is done, 0 when stopped, -1 on another error.
#[unsafe(no_mangle)]
pub extern "C" fn grow_table(bits: u32) -> i32 {
    let mut s = STATE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(lua) = s.as_mut() else { return -1 };
    READS.store(0, Relaxed);
    match lua.exec(&format!("T[(1 << {bits}) + 1] = 1 return #T"), "=grow") {
        Ok(v) if matches!(v[..], [Value::Int(_)]) => 1,
        Err(remedy_lua::Error::Limit(remedy_lua::LimitKind::Time)) => 0,
        _ => -1,
    }
}

/// Reads and writes slots `lo..hi` of `T` `reps` times (1 when done).
#[unsafe(no_mangle)]
pub extern "C" fn rw(lo: u32, hi: u32, reps: u32) -> i32 {
    let mut s = STATE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(lua) = s.as_mut() else { return -1 };
    EVERY.store(0, Relaxed);
    let src = format!("local T, x = T, 0 for r = 1, {reps} do for i = {lo}, {hi} do x = x + T[i] T[i] = x % 7 end end return x");
    i32::from(lua.exec(&src, "=rw").is_ok())
}

/// The table's slots copied by growths and the array parts made segmented since the last
/// call, packed: copied * 16 + segmented (a test counter).
#[unsafe(no_mangle)]
pub extern "C" fn copied() -> u32 {
    let mut s = STATE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(lua) = s.as_mut() else { return u32::MAX };
    let (c, g) = lua.table_slots_copied();
    (c as u32).saturating_mul(16).saturating_add(g as u32)
}

/// Slots `1..=n` of `T` set (no stops), so a read/write loop reads values.
#[unsafe(no_mangle)]
pub extern "C" fn fill(n: u32) -> i32 {
    let mut s = STATE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(lua) = s.as_mut() else { return -1 };
    EVERY.store(0, Relaxed);
    i32::from(lua.exec(&format!("for i = 1, {n} do T[i] = i end"), "=fill").is_ok())
}

static BLOCKS: Mutex<Vec<Box<[Value]>>> = Mutex::new(Vec::new());

/// One block of `slots` nil values made and kept (a segmented array part's growth step).
#[unsafe(no_mangle)]
pub extern "C" fn alloc_block(slots: u32) -> i32 {
    let mut b = BLOCKS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    b.push(vec![Value::Nil; slots as usize].into_boxed_slice());
    b.len() as i32
}
