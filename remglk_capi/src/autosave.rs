/*

Autosave/autorestore C API
==========================

Copyright (c) 2026 Dannii Willis
MIT licenced
https://github.com/curiousdannii/remglk-rs

*/

use std::ffi::{c_char, c_int, c_void, CStr};
use std::ptr;

use remglk::glkapi::autosave::LibraryState;
use remglk::glkapi::objects::DispatchRock;
use serde_json::{Map, Value};

use super::common::*;
use super::glkapi::*;

const MAX_CALLBACK_LIST_ENTRIES: c_int = 4096;

type SerializeObjectCallback = extern "C" fn(*mut SerializeContext, *mut c_void) -> c_int;
type UnserializeObjectCallback = extern "C" fn(*mut UnserializeContext, *mut c_void) -> c_int;

#[repr(C)]
pub struct SerializeContext {
    failed: bool,
    value: Map<String, Value>,
}

#[repr(C)]
pub struct UnserializeContext {
    children: Vec<*mut UnserializeContext>,
    value: Value,
}

impl UnserializeContext {
    fn new(value: Value) -> Self {
        Self {
            children: Vec::new(),
            value,
        }
    }

    unsafe fn free_children(&mut self) {
        for child in self.children.drain(..) {
            let mut child = Box::from_raw(child);
            child.free_children();
        }
    }
}

impl SerializeContext {
    fn new() -> Self {
        Self {
            failed: false,
            value: Map::new(),
        }
    }

    fn name(&mut self, name: *const c_char) -> Option<String> {
        if name.is_null() {
            self.failed = true;
            return None;
        }
        match unsafe { CStr::from_ptr(name) }.to_str() {
            Ok(name) => Some(name.to_string()),
            Err(_) => {
                self.failed = true;
                None
            }
        }
    }
}

fn serialize_extra_state(
    callback: Option<SerializeObjectCallback>,
    rock: *mut c_void,
) -> Result<Option<Value>, String> {
    let Some(callback) = callback else {
        return Ok(None);
    };
    let mut context = SerializeContext::new();
    if callback(&mut context, rock) == 0 || context.failed {
        return Err("extra-state serialization callback failed".into());
    }
    Ok(Some(Value::Object(context.value)))
}

#[no_mangle]
pub unsafe extern "C" fn glkunix_serialize_uint32(
    context: *mut SerializeContext,
    name: *const c_char,
    value: u32,
) {
    let Some(context) = context.as_mut() else {
        return;
    };
    if let Some(name) = context.name(name) {
        context.value.insert(name, Value::from(value));
    }
}

#[no_mangle]
pub unsafe extern "C" fn glkunix_serialize_object(
    context: *mut SerializeContext,
    name: *const c_char,
    callback: Option<SerializeObjectCallback>,
    rock: *mut c_void,
) {
    let Some(context) = context.as_mut() else {
        return;
    };
    let Some(name) = context.name(name) else {
        return;
    };
    let Some(callback) = callback else {
        context.failed = true;
        return;
    };
    let mut child = SerializeContext::new();
    if callback(&mut child, rock) == 0 || child.failed {
        context.failed = true;
        return;
    }
    context.value.insert(name, Value::Object(child.value));
}

#[no_mangle]
pub unsafe extern "C" fn glkunix_serialize_object_list(
    context: *mut SerializeContext,
    name: *const c_char,
    callback: Option<SerializeObjectCallback>,
    count: c_int,
    element_size: usize,
    elements: *mut c_void,
) {
    let Some(context) = context.as_mut() else {
        return;
    };
    let Some(name) = context.name(name) else {
        return;
    };
    let Some(callback) = callback else {
        context.failed = true;
        return;
    };
    if !(0..=MAX_CALLBACK_LIST_ENTRIES).contains(&count)
        || (count > 0 && (element_size == 0 || elements.is_null()))
    {
        context.failed = true;
        return;
    }
    let mut list = Vec::with_capacity(count as usize);
    for index in 0..count as usize {
        let mut child = SerializeContext::new();
        let rock = elements.cast::<u8>().add(index * element_size).cast();
        if callback(&mut child, rock) == 0 || child.failed {
            context.failed = true;
            return;
        }
        list.push(Value::Object(child.value));
    }
    context.value.insert(name, Value::Array(list));
}

#[no_mangle]
pub unsafe extern "C" fn glkunix_unserialize_uint32(
    context: *const UnserializeContext,
    name: *const c_char,
    result: *mut u32,
) -> c_int {
    let Some(context) = context.as_ref() else {
        return 0;
    };
    if name.is_null() {
        return 0;
    }
    let Ok(name) = CStr::from_ptr(name).to_str() else {
        return 0;
    };
    let Some(value) = context
        .value
        .as_object()
        .and_then(|object| object.get(name))
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
    else {
        return 0;
    };
    write_ptr(result, value);
    1
}

#[no_mangle]
pub unsafe extern "C" fn glkunix_unserialize_struct(
    context: *mut UnserializeContext,
    name: *const c_char,
    result: *mut *mut UnserializeContext,
) -> c_int {
    let Some(context) = context.as_mut() else {
        return 0;
    };
    if name.is_null() {
        return 0;
    }
    let Ok(name) = CStr::from_ptr(name).to_str() else {
        return 0;
    };
    let Some(value) = context
        .value
        .as_object()
        .and_then(|object| object.get(name))
        .filter(|value| value.is_object())
    else {
        return 0;
    };
    let child = Box::into_raw(Box::new(UnserializeContext::new(value.clone())));
    context.children.push(child);
    write_ptr(result, child);
    1
}

#[no_mangle]
pub unsafe extern "C" fn glkunix_unserialize_list(
    context: *mut UnserializeContext,
    name: *const c_char,
    result: *mut *mut UnserializeContext,
    count: *mut c_int,
) -> c_int {
    let Some(context) = context.as_mut() else {
        return 0;
    };
    if name.is_null() {
        return 0;
    }
    let Ok(name) = CStr::from_ptr(name).to_str() else {
        return 0;
    };
    let Some(list) = context
        .value
        .as_object()
        .and_then(|object| object.get(name))
        .and_then(Value::as_array)
    else {
        return 0;
    };
    if list.len() > MAX_CALLBACK_LIST_ENTRIES as usize {
        return 0;
    }
    let child = Box::into_raw(Box::new(UnserializeContext::new(Value::Array(
        list.clone(),
    ))));
    context.children.push(child);
    write_ptr(count, list.len() as c_int);
    write_ptr(result, child);
    1
}

#[no_mangle]
pub unsafe extern "C" fn glkunix_unserialize_list_entry(
    context: *mut UnserializeContext,
    index: c_int,
    result: *mut *mut UnserializeContext,
) -> c_int {
    let Some(context) = context.as_mut() else {
        return 0;
    };
    let Ok(index) = usize::try_from(index) else {
        return 0;
    };
    let Some(value) = context
        .value
        .as_array()
        .and_then(|list| list.get(index))
        .filter(|value| value.is_object())
    else {
        return 0;
    };
    let child = Box::into_raw(Box::new(UnserializeContext::new(value.clone())));
    context.children.push(child);
    write_ptr(result, child);
    1
}

#[no_mangle]
pub unsafe extern "C" fn glkunix_unserialize_object_list_entries(
    context: *mut UnserializeContext,
    callback: Option<UnserializeObjectCallback>,
    count: c_int,
    element_size: usize,
    elements: *mut c_void,
) -> c_int {
    let Some(context) = context.as_ref() else {
        return 0;
    };
    let Some(callback) = callback else { return 0 };
    let Some(list) = context.value.as_array() else {
        return 0;
    };
    if count < 0
        || count as usize != list.len()
        || list.len() > MAX_CALLBACK_LIST_ENTRIES as usize
        || (count > 0 && (element_size == 0 || elements.is_null()))
    {
        return 0;
    }
    for (index, value) in list.iter().enumerate() {
        if !value.is_object() {
            return 0;
        }
        let mut child = UnserializeContext::new(value.clone());
        let rock = elements.cast::<u8>().add(index * element_size).cast();
        let succeeded = callback(&mut child, rock) != 0;
        child.free_children();
        if !succeeded {
            return 0;
        }
    }
    1
}

#[no_mangle]
pub extern "C" fn glkunix_save_library_state(
    file: StreamPtr,
    omitstream: StreamPtr,
    extra_state_callback: Option<SerializeObjectCallback>,
    extra_state_rock: *mut c_void,
) {
    let result = (|| {
        let extra_state = serialize_extra_state(extra_state_callback, extra_state_rock)?;
        let file = from_ptr(file, "glkunix_save_library_state");
        let omitstream = from_ptr_opt(omitstream);
        let glkapi = GLKAPI.lock().unwrap();
        let data = glkapi
            .save_library_state(omitstream.as_ref(), extra_state)
            .map_err(|error| error.to_string())?;
        glkapi
            .write_library_state(&file, &data)
            .map_err(|error| error.to_string())
    })();
    if let Err(error) = result {
        eprintln!("RemGlk-rs autosave failed: {error}");
    }
}

#[no_mangle]
pub extern "C" fn glkunix_load_library_state(
    file: StreamPtr,
    extra_state_callback: Option<UnserializeObjectCallback>,
    extra_state_rock: *mut c_void,
) -> *mut LibraryState {
    let result = (|| {
        let file = from_ptr(file, "glkunix_load_library_state");
        let state = GLKAPI
            .lock()
            .unwrap()
            .read_library_state(&file)
            .map_err(|error| error.to_string())?;
        if let Some(callback) = extra_state_callback {
            let extra = state
                .extra_state()
                .filter(|value| value.is_object())
                .cloned()
                .ok_or_else(|| "extra state is missing or not an object".to_string())?;
            let mut context = UnserializeContext::new(extra);
            let succeeded = callback(&mut context, extra_state_rock) != 0;
            unsafe { context.free_children() };
            if !succeeded {
                return Err("extra-state unserialization callback failed".to_string());
            }
        }
        Ok(Box::into_raw(Box::new(state)))
    })();
    match result {
        Ok(state) => state,
        Err(error) => {
            eprintln!("RemGlk-rs autorestore load failed: {error}");
            ptr::null_mut()
        }
    }
}

#[no_mangle]
pub extern "C" fn glkunix_update_from_library_state(state: *mut LibraryState) -> u32 {
    let Some(state) = (unsafe { state.as_mut() }) else {
        return 0;
    };
    match GLKAPI.lock().unwrap().update_from_library_state(state) {
        Ok(()) => 1,
        Err(error) => {
            eprintln!("RemGlk-rs autorestore update failed: {error}");
            0
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn glkunix_library_state_free(state: *mut LibraryState) {
    if !state.is_null() {
        drop(Box::from_raw(state));
    }
}

#[no_mangle]
pub extern "C" fn glkunix_get_last_event_type() -> u32 {
    GLKAPI.lock().unwrap().glkunix_get_last_event_type()
}

#[no_mangle]
pub extern "C" fn glkunix_window_get_updatetag(window: WindowPtr) -> u32 {
    lock!(from_ptr(window, "glkunix_window_get_updatetag")).id
}

#[no_mangle]
pub extern "C" fn glkunix_window_find_by_updatetag(tag: u32) -> WindowPtr {
    borrow(GLKAPI.lock().unwrap().windows.get_by_id(tag).as_ref())
}

#[no_mangle]
pub extern "C" fn glkunix_window_set_dispatch_rock(window: WindowPtr, rock: DispatchRock) {
    let window = from_ptr(window, "glkunix_window_set_dispatch_rock");
    GLKAPI
        .lock()
        .unwrap()
        .windows
        .set_dispatch_rock(&window, rock);
}

#[no_mangle]
pub extern "C" fn glkunix_stream_get_updatetag(stream: StreamPtr) -> u32 {
    lock!(from_ptr(stream, "glkunix_stream_get_updatetag")).id
}

#[no_mangle]
pub extern "C" fn glkunix_stream_find_by_updatetag(tag: u32) -> StreamPtr {
    borrow(GLKAPI.lock().unwrap().streams.get_by_id(tag).as_ref())
}

#[no_mangle]
pub extern "C" fn glkunix_stream_set_dispatch_rock(stream: StreamPtr, rock: DispatchRock) {
    let stream = from_ptr(stream, "glkunix_stream_set_dispatch_rock");
    GLKAPI
        .lock()
        .unwrap()
        .streams
        .set_dispatch_rock(&stream, rock);
}

#[no_mangle]
pub extern "C" fn glkunix_fileref_get_updatetag(fileref: FileRefPtr) -> u32 {
    lock!(from_ptr(fileref, "glkunix_fileref_get_updatetag")).id
}

#[no_mangle]
pub extern "C" fn glkunix_fileref_find_by_updatetag(tag: u32) -> FileRefPtr {
    borrow(GLKAPI.lock().unwrap().filerefs.get_by_id(tag).as_ref())
}

#[no_mangle]
pub extern "C" fn glkunix_fileref_set_dispatch_rock(fileref: FileRefPtr, rock: DispatchRock) {
    let fileref = from_ptr(fileref, "glkunix_fileref_set_dispatch_rock");
    GLKAPI
        .lock()
        .unwrap()
        .filerefs
        .set_dispatch_rock(&fileref, rock);
}

#[cfg(test)]
mod tests;
