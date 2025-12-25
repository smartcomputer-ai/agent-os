#![cfg_attr(not(any(test, feature = "std")), no_std)]

extern crate alloc;

mod pure;
mod reducers;

pub use pure::*;
pub use reducers::*;

pub use serde_cbor::Value;

use alloc::alloc::{alloc, Layout};
use core::slice;

#[inline]
fn guest_alloc(len: usize) -> *mut u8 {
    if len == 0 {
        return core::ptr::null_mut();
    }
    let layout = Layout::from_size_align(len, 8).expect("layout");
    unsafe { alloc(layout) }
}

unsafe fn read_input<'a>(ptr: i32, len: i32) -> &'a [u8] {
    if ptr == 0 || len <= 0 {
        return &[];
    }
    unsafe { slice::from_raw_parts(ptr as *const u8, len as usize) }
}

fn write_back(bytes: &[u8]) -> (i32, i32) {
    let len = bytes.len();
    if len == 0 {
        return (0, 0);
    }
    let ptr = guest_alloc(len);
    unsafe {
        let out = slice::from_raw_parts_mut(ptr as *mut u8, len);
        out.copy_from_slice(bytes);
    }
    (ptr as i32, len as i32)
}

/// Exported allocator body (used by the macro-generated `alloc`).
pub fn exported_alloc(len: i32) -> i32 {
    if len <= 0 {
        return 0;
    }
    guest_alloc(len as usize) as i32
}
