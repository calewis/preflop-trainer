//! WebAssembly bridge for the trainer. JavaScript sends one command string in
//! and gets one JSON string out; there is no other surface. The clock and the
//! storage belong to JavaScript and arrive here as plain arguments.

mod engine;
mod json;
mod present;

pub use engine::dispatch;

#[cfg(target_arch = "wasm32")]
mod ffi {
    use std::cell::RefCell;

    thread_local! {
        static RESULT: RefCell<String> = const { RefCell::new(String::new()) };
    }

    #[link(wasm_import_module = "env")]
    extern "C" {
        fn tr_panic(ptr: *const u8, len: usize);
    }

    /// Reports a panic message to JavaScript before the trap that follows it.
    #[no_mangle]
    pub extern "C" fn tr_init() {
        std::panic::set_hook(Box::new(|info| {
            let text = info.to_string();
            unsafe { tr_panic(text.as_ptr(), text.len()) };
        }));
    }

    #[no_mangle]
    pub extern "C" fn tr_alloc(len: usize) -> *mut u8 {
        let mut buffer = Vec::<u8>::with_capacity(len.max(1));
        let ptr = buffer.as_mut_ptr();
        std::mem::forget(buffer);
        ptr
    }

    /// Runs the command in the `len` bytes at `ptr` and frees them. Returns the
    /// length of the JSON result, readable at [`tr_result_ptr`] until the next call.
    #[no_mangle]
    pub extern "C" fn tr_call(ptr: *mut u8, len: usize) -> usize {
        let bytes = unsafe { Vec::from_raw_parts(ptr, len, len.max(1)) };
        let input = String::from_utf8_lossy(&bytes);
        let output = crate::dispatch(&input);
        RESULT.with(|slot| {
            let mut slot = slot.borrow_mut();
            *slot = output;
            slot.len()
        })
    }

    #[no_mangle]
    pub extern "C" fn tr_result_ptr() -> *const u8 {
        RESULT.with(|slot| slot.borrow().as_ptr())
    }
}
