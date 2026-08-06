use crate::ipc::UserWindowEvent;
use std::sync::Arc;
use std::task::{RawWaker, RawWakerVTable, Waker};
use tao::{event_loop::EventLoopProxy, window::WindowId};

struct DomHandle {
    proxy: EventLoopProxy<UserWindowEvent>,
    id: WindowId,
}

// this should be implemented by most platforms, but ios is missing this until
// https://github.com/tauri-apps/wry/issues/830 is resolved
unsafe impl Send for DomHandle {}
unsafe impl Sync for DomHandle {}

static DOM_HANDLE_VTABLE: RawWakerVTable = RawWakerVTable::new(
    clone_raw,
    wake_raw,
    wake_by_ref_raw,
    drop_raw,
);

unsafe fn clone_raw(ptr: *const ()) -> RawWaker {
    Arc::increment_strong_count(ptr as *const DomHandle);
    RawWaker::new(ptr, &DOM_HANDLE_VTABLE)
}

unsafe fn wake_raw(ptr: *const ()) {
    wake_by_ref_raw(ptr);
    drop_raw(ptr);
}

unsafe fn wake_by_ref_raw(ptr: *const ()) {
    let handle = &*(ptr as *const DomHandle);
    let _ = handle.proxy.send_event(UserWindowEvent::Poll(handle.id));
}

unsafe fn drop_raw(ptr: *const ()) {
    drop(Arc::from_raw(ptr as *const DomHandle));
}

/// Create a waker that will send a poll event to the event loop.
///
/// This lets the VirtualDom "come up for air" and process events while the main thread is blocked by the WebView.
///
/// All IO and multithreading lives on other threads. Thanks to tokio's work stealing approach, the main thread can never
/// claim a task while it's blocked by the event loop.
pub fn tao_waker(proxy: EventLoopProxy<UserWindowEvent>, id: WindowId) -> Waker {
    let handle = Arc::new(DomHandle { id, proxy });
    let ptr = Arc::into_raw(handle) as *const ();
    let raw_waker = RawWaker::new(ptr, &DOM_HANDLE_VTABLE);
    unsafe { Waker::from_raw(raw_waker) }
}
