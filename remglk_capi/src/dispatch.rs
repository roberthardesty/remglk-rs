/*

GiDispatch functions
====================

Copyright (c) 2024 Dannii Willis
MIT licenced
https://github.com/curiousdannii/remglk-rs

*/

use std::ffi::{c_int, c_long, c_void};
use std::mem;

use objects::*;
use remglk::glkapi::*;

use super::*;
use common::*;
use glkapi::*;

type RegisterCallbackGeneric = extern "C" fn(*const c_void, u32) -> DispatchRock;
type UnregisterCallbackGeneric = extern "C" fn(*const c_void, u32, DispatchRock);

#[no_mangle]
pub unsafe extern "C" fn gidispatch_set_object_registry(
    register_cb: RegisterCallbackGeneric,
    unregister_cb: UnregisterCallbackGeneric,
) {
    let mut glkapi = GLKAPI.lock().unwrap();
    let register = mem::transmute::<RegisterCallbackGeneric, DispatchRegisterCallback<GlkFileRef>>(
        register_cb,
    );
    let unregister = mem::transmute::<
        UnregisterCallbackGeneric,
        DispatchUnregisterCallback<GlkFileRef>,
    >(unregister_cb);
    glkapi.filerefs.set_callbacks(register, unregister);
    let register = mem::transmute::<
        RegisterCallbackGeneric,
        DispatchRegisterCallback<GlkSoundChannel>,
    >(register_cb);
    let unregister = mem::transmute::<
        UnregisterCallbackGeneric,
        DispatchUnregisterCallback<GlkSoundChannel>,
    >(unregister_cb);
    glkapi.schannels.set_callbacks(register, unregister);
    let register =
        mem::transmute::<RegisterCallbackGeneric, DispatchRegisterCallback<GlkStream>>(register_cb);
    let unregister = mem::transmute::<
        UnregisterCallbackGeneric,
        DispatchUnregisterCallback<GlkStream>,
    >(unregister_cb);
    glkapi.streams.set_callbacks(register, unregister);
    let register =
        mem::transmute::<RegisterCallbackGeneric, DispatchRegisterCallback<GlkWindow>>(register_cb);
    let unregister = mem::transmute::<
        UnregisterCallbackGeneric,
        DispatchUnregisterCallback<GlkWindow>,
    >(unregister_cb);
    glkapi.windows.set_callbacks(register, unregister);
}

// The C function `gidispatch_get_objrock` takes a generic pointer, which we can't really deal with here in Rust, so support.c will handle calling the appropriate function
#[no_mangle]
pub extern "C" fn gidispatch_get_objrock_fileref(ptr: FileRefPtr) -> DispatchRock {
    let obj = from_ptr(ptr, "gidispatch_get_objrock_fileref");
    let obj = obj.lock().unwrap();
    obj.disprock.unwrap()
}

#[no_mangle]
pub extern "C" fn gidispatch_get_objrock_schannel(ptr: SchannelPtr) -> DispatchRock {
    let obj = from_ptr(ptr, "gidispatch_get_objrock_schannel");
    let obj = obj.lock().unwrap();
    obj.disprock.unwrap()
}

#[no_mangle]
pub extern "C" fn gidispatch_get_objrock_stream(ptr: StreamPtr) -> DispatchRock {
    let obj = from_ptr(ptr, "gidispatch_get_objrock_stream");
    let obj = obj.lock().unwrap();
    obj.disprock.unwrap()
}

#[no_mangle]
pub extern "C" fn gidispatch_get_objrock_window(ptr: WindowPtr) -> DispatchRock {
    let obj = from_ptr(ptr, "gidispatch_get_objrock_window");
    let obj = obj.lock().unwrap();
    obj.disprock.unwrap()
}

type LocateArrayCallbackGeneric =
    extern "C" fn(*const c_void, u32, *const c_char, DispatchRock, *mut c_int) -> c_long;
type RestoreArrayCallbackGeneric =
    extern "C" fn(c_long, u32, *const c_char, *mut *mut c_void) -> DispatchRock;
type RetainArrayCallbackGeneric = extern "C" fn(*const c_void, u32, *const c_char) -> DispatchRock;
type UnretainArrayCallbackGeneric = extern "C" fn(*const c_void, u32, *const c_char, DispatchRock);

#[no_mangle]
pub unsafe extern "C" fn gidispatch_set_autorestore_registry(
    locate_cb: LocateArrayCallbackGeneric,
    restore_cb: RestoreArrayCallbackGeneric,
) {
    let mut glkapi = GLKAPI.lock().unwrap();
    glkapi.locate_array_callbacks_u8 = Some(mem::transmute::<
        LocateArrayCallbackGeneric,
        LocateArrayCallback<u8>,
    >(locate_cb));
    glkapi.locate_array_callbacks_u32 = Some(mem::transmute::<
        LocateArrayCallbackGeneric,
        LocateArrayCallback<u32>,
    >(locate_cb));
    glkapi.restore_array_callbacks_u8 = Some(mem::transmute::<
        RestoreArrayCallbackGeneric,
        RestoreArrayCallback<u8>,
    >(restore_cb));
    glkapi.restore_array_callbacks_u32 = Some(mem::transmute::<
        RestoreArrayCallbackGeneric,
        RestoreArrayCallback<u32>,
    >(restore_cb));
}

#[no_mangle]
pub unsafe extern "C" fn gidispatch_set_retained_registry(
    register_cb: RetainArrayCallbackGeneric,
    unregister_cb: UnretainArrayCallbackGeneric,
) {
    let mut glkapi = GLKAPI.lock().unwrap();
    let retain = mem::transmute::<RetainArrayCallbackGeneric, RetainArrayCallback<u8>>(register_cb);
    let unretain =
        mem::transmute::<UnretainArrayCallbackGeneric, UnretainArrayCallback<u8>>(unregister_cb);
    glkapi.retain_array_callbacks_u8 = Some(RetainArrayCallbacks { retain, unretain });
    let retain =
        mem::transmute::<RetainArrayCallbackGeneric, RetainArrayCallback<u32>>(register_cb);
    let unretain =
        mem::transmute::<UnretainArrayCallbackGeneric, UnretainArrayCallback<u32>>(unregister_cb);
    glkapi.retain_array_callbacks_u32 = Some(RetainArrayCallbacks { retain, unretain });
}
