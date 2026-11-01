use std::sync::{LazyLock, Mutex};

use jiff::tz::TimeZone;

use super::*;

static TEST_LOCK: Mutex<()> = Mutex::new(());
static ARRAYS: LazyLock<Mutex<ArrayRegistry>> =
    LazyLock::new(|| Mutex::new(ArrayRegistry::default()));

#[derive(Default)]
struct ArrayRegistry {
    arrays: HashMap<usize, (u32, usize)>,
    next_address: u32,
    writes: Vec<(u32, Vec<u32>)>,
}

impl ArrayRegistry {
    fn reset(&mut self) {
        self.arrays.clear();
        self.next_address = 0x1000;
        self.writes.clear();
    }

    fn register(&mut self, pointer: usize, len: usize, address: Option<u32>) -> DispatchRock {
        let address = address.unwrap_or_else(|| {
            let address = self.next_address;
            self.next_address += 4;
            address
        });
        self.arrays.insert(pointer, (address, len));
        DispatchRock { num: address }
    }
}

extern "C" fn retain_u8(pointer: *const u8, len: u32, _: *const c_char) -> DispatchRock {
    ARRAYS
        .lock()
        .unwrap()
        .register(pointer as usize, len as usize, None)
}

extern "C" fn retain_u32(pointer: *const u32, len: u32, _: *const c_char) -> DispatchRock {
    ARRAYS
        .lock()
        .unwrap()
        .register(pointer as usize, len as usize, None)
}

extern "C" fn unretain_u8(pointer: *const u8, len: u32, _: *const c_char, rock: DispatchRock) {
    let data = unsafe { slice::from_raw_parts(pointer, len as usize) };
    let address = unsafe { rock.num };
    ARRAYS
        .lock()
        .unwrap()
        .writes
        .push((address, data.iter().map(|&value| value as u32).collect()));
    ARRAYS.lock().unwrap().arrays.remove(&(pointer as usize));
    unsafe {
        drop(Box::from_raw(slice::from_raw_parts_mut(
            pointer as *mut u8,
            len as usize,
        )))
    };
}

extern "C" fn unretain_u32(pointer: *const u32, len: u32, _: *const c_char, rock: DispatchRock) {
    let data = unsafe { slice::from_raw_parts(pointer, len as usize) };
    let address = unsafe { rock.num };
    ARRAYS.lock().unwrap().writes.push((address, data.to_vec()));
    ARRAYS.lock().unwrap().arrays.remove(&(pointer as usize));
    unsafe {
        drop(Box::from_raw(slice::from_raw_parts_mut(
            pointer as *mut u32,
            len as usize,
        )))
    };
}

extern "C" fn locate_u8(
    pointer: *const u8,
    len: u32,
    _: *const c_char,
    _: DispatchRock,
    element_size: *mut c_int,
) -> c_long {
    locate(pointer as usize, len, element_size, 1)
}

extern "C" fn locate_u32(
    pointer: *const u32,
    len: u32,
    _: *const c_char,
    _: DispatchRock,
    element_size: *mut c_int,
) -> c_long {
    locate(pointer as usize, len, element_size, 4)
}

fn locate(pointer: usize, len: u32, element_size: *mut c_int, expected_size: c_int) -> c_long {
    let arrays = ARRAYS.lock().unwrap();
    let &(address, registered_len) = arrays.arrays.get(&pointer).unwrap();
    assert_eq!(registered_len, len as usize);
    unsafe { *element_size = expected_size };
    address as c_long
}

extern "C" fn restore_u8(
    address: c_long,
    len: u32,
    _: *const c_char,
    arrayref: *mut *mut u8,
) -> DispatchRock {
    let mut data = vec![0; len as usize].into_boxed_slice();
    let pointer = data.as_mut_ptr();
    unsafe { *arrayref = pointer };
    Box::leak(data);
    ARRAYS
        .lock()
        .unwrap()
        .register(pointer as usize, len as usize, Some(address as u32))
}

extern "C" fn restore_u32(
    address: c_long,
    len: u32,
    _: *const c_char,
    arrayref: *mut *mut u32,
) -> DispatchRock {
    let mut data = vec![0; len as usize].into_boxed_slice();
    let pointer = data.as_mut_ptr();
    unsafe { *arrayref = pointer };
    Box::leak(data);
    ARRAYS
        .lock()
        .unwrap()
        .register(pointer as usize, len as usize, Some(address as u32))
}

extern "C" fn restore_fail(_: c_long, _: u32, _: *const c_char, _: *mut *mut u32) -> DispatchRock {
    DispatchRock { num: 0 }
}

#[derive(Default)]
struct TestSystem {
    files: HashMap<String, Box<[u8]>>,
}

impl GlkSystem for TestSystem {
    fn file_delete(&mut self, path: &str) {
        self.files.remove(path);
    }

    fn file_delete_sync(&mut self, path: &str) {
        self.files.remove(path);
    }

    fn file_exists(&mut self, path: &str) -> bool {
        self.files.contains_key(path)
    }

    fn file_read(&mut self, path: &str) -> Option<Box<[u8]>> {
        self.files.get(path).cloned()
    }

    fn file_write_buffer(&mut self, path: &str, buf: Box<[u8]>) {
        self.files.insert(path.to_string(), buf);
    }

    fn send_glkote_update(&mut self, _: Update, _: bool) {}

    fn get_glkote_event(&mut self) -> Option<Event> {
        None
    }

    fn buffer_canon_decompose(buf: &mut [u32], initlen: usize) -> usize {
        buf[..initlen].len()
    }

    fn buffer_canon_normalize(buf: &mut [u32], initlen: usize) -> usize {
        buf[..initlen].len()
    }

    fn buffer_to_lower_case(_: &mut [u32], initlen: usize) -> usize {
        initlen
    }

    fn buffer_to_title_case(_: &mut [u32], initlen: usize, _: bool) -> usize {
        initlen
    }

    fn buffer_to_upper_case(_: &mut [u32], initlen: usize) -> usize {
        initlen
    }

    fn get_directories() -> Directories {
        Directories::default()
    }

    fn get_local_tz() -> TimeZone {
        TimeZone::UTC
    }

    fn get_now() -> Timestamp {
        Timestamp::new(0, 0).unwrap()
    }

    fn set_base_file(_: &mut Directories, _: String) {}
}

fn test_api() -> GlkApi<TestSystem> {
    ARRAYS.lock().unwrap().reset();
    let mut api = GlkApi::new(TestSystem::default());
    api.retain_array_callbacks_u8 = Some(RetainArrayCallbacks {
        retain: retain_u8,
        unretain: unretain_u8,
    });
    api.retain_array_callbacks_u32 = Some(RetainArrayCallbacks {
        retain: retain_u32,
        unretain: unretain_u32,
    });
    api.locate_array_callbacks_u8 = Some(locate_u8);
    api.locate_array_callbacks_u32 = Some(locate_u32);
    api.restore_array_callbacks_u8 = Some(restore_u8);
    api.restore_array_callbacks_u32 = Some(restore_u32);
    api.handle_event(Event {
        gen: 0,
        partial: None,
        data: EventData::Init(InitEvent {
            metrics: Box::new(Metrics {
                buffercharheight: Some(1.0),
                buffercharwidth: Some(1.0),
                gridcharheight: Some(1.0),
                gridcharwidth: Some(1.0),
                height: 50.0,
                width: 80.0,
                ..Default::default()
            }),
            support: vec!["timer".into()],
        }),
    })
    .unwrap();
    api
}

fn line_event(window: u32, gen: u32, value: &str) -> Event {
    Event {
        gen,
        partial: None,
        data: EventData::Line(LineEvent {
            terminator: None,
            value: value.into(),
            window,
        }),
    }
}

fn blorb_with_data(resource: u32, data: &[u8]) -> Box<[u8]> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"FORM");
    bytes.extend_from_slice(&(36 + data.len() as u32).to_be_bytes());
    bytes.extend_from_slice(b"IFRS");
    bytes.extend_from_slice(b"RIdx");
    bytes.extend_from_slice(&16u32.to_be_bytes());
    bytes.extend_from_slice(&1u32.to_be_bytes());
    bytes.extend_from_slice(b"Data");
    bytes.extend_from_slice(&resource.to_be_bytes());
    bytes.extend_from_slice(&36u32.to_be_bytes());
    bytes.extend_from_slice(b"BINA");
    bytes.extend_from_slice(&(data.len() as u32).to_be_bytes());
    bytes.extend_from_slice(data);
    assert_eq!(bytes.len(), 48 + data.len() - 4);
    bytes.into_boxed_slice()
}

#[test]
fn round_trip_restores_unicode_buffers_tags_and_one_full_refresh() {
    let _guard = TEST_LOCK.lock().unwrap();
    let mut api = test_api();
    let root = api
        .glk_window_open(None, 0, 0, WindowType::Buffer, 42)
        .unwrap();
    let c_root = root.clone();
    let window_tag = lock!(root).id;
    api.glk_set_window(Some(&root));
    api.glk_put_buffer(b"before\n").unwrap();
    api.glk_request_line_event_uni(&mut lock!(root), vec![0; 16].into_boxed_slice(), 3)
        .unwrap();
    let input_address = unsafe { lock!(root).array_disprock.unwrap().num };
    drop(root);

    let memory = api
        .glk_stream_open_memory_uni(
            Some(vec![65, 0x1F642, 0].into_boxed_slice()),
            FileMode::ReadWrite,
            77,
        )
        .unwrap();
    let c_memory = memory.clone();
    let memory_tag = lock!(memory).id;
    GlkApi::<TestSystem>::glk_stream_set_position(&mut lock!(memory), 1, SeekMode::Start);
    api.glk_stream_set_current(Some(&memory));
    api.glk_request_timer_events(250);
    drop(memory);

    let json = api
        .save_library_state(None, Some(serde_json::json!({"glulx_extra_state": 1})))
        .unwrap();
    let mut state = LibraryState::from_json(&json).unwrap();
    api.update_from_library_state(&mut state).unwrap();
    drop(c_root);
    drop(c_memory);
    assert!(matches!(
        api.update_from_library_state(&mut state),
        Err(AutosaveError::Consumed)
    ));
    assert_eq!(api.glkunix_get_last_event_type(), RESTORED_EVENT_TYPE);
    assert_eq!(lock!(api.glk_stream_get_current().unwrap()).id, memory_tag);
    assert_eq!(api.timer.interval, 250);

    let update = api.update();
    let Update::State(update) = update else {
        panic!("expected state update")
    };
    assert_eq!(update.windows.len(), 1);
    assert_eq!(update.content.len(), 1);
    assert_eq!(update.input.len(), 1);
    assert_eq!(update.input[0].id, window_tag);
    assert_eq!(update.timer, Some(250));
    let second = api.update();
    let Update::State(second) = second else {
        panic!("expected state update")
    };
    assert!(second.windows.is_empty());
    assert!(second.content.is_empty());

    let memory = api.streams.get_by_id(memory_tag).unwrap();
    assert_eq!(
        GlkApi::<TestSystem>::glk_stream_get_position(&mut lock!(memory)),
        1
    );
    assert_eq!(
        GlkApi::<TestSystem>::glk_get_char_stream_uni(&mut lock!(memory)).unwrap(),
        0x1F642
    );
    drop(memory);

    api.handle_event(line_event(window_tag, 1, "λ🙂")).unwrap();
    let writes = &ARRAYS.lock().unwrap().writes;
    let (address, data) = writes.last().unwrap();
    assert_eq!(*address, input_address);
    assert_eq!(&data[..2], &[0x3BB, 0x1F642]);

    let root = api.glk_window_get_root().unwrap();
    let new_window = api
        .glk_window_open(
            Some(&root),
            winmethod_Below | winmethod_Proportional,
            50,
            WindowType::Grid,
            7,
        )
        .unwrap();
    assert!(lock!(new_window).id > window_tag);
}

#[test]
fn malformed_schema_and_size_are_rejected_before_live_changes() {
    let _guard = TEST_LOCK.lock().unwrap();
    let mut api = test_api();
    let root = api
        .glk_window_open(None, 0, 0, WindowType::Buffer, 1)
        .unwrap();
    let root_pointer = root.as_ptr();
    drop(root);
    let json = api.save_library_state(None, None).unwrap();

    assert!(LibraryState::from_json(b"{").is_err());
    assert!(LibraryState::from_json(&vec![b' '; MAX_LIBRARY_STATE_BYTES + 1]).is_err());
    let mut value: Value = serde_json::from_slice(&json).unwrap();
    value["version"] = Value::from(2);
    assert!(matches!(
        LibraryState::from_json(&serde_json::to_vec(&value).unwrap()),
        Err(AutosaveError::UnsupportedVersion)
    ));
    value = serde_json::from_slice(&json).unwrap();
    value["windows"][0]["stream_tag"] = Value::from(9999);
    assert!(LibraryState::from_json(&serde_json::to_vec(&value).unwrap()).is_err());
    value = serde_json::from_slice(&json).unwrap();
    value["unexpected"] = Value::from(true);
    assert!(LibraryState::from_json(&serde_json::to_vec(&value).unwrap()).is_err());

    let root = api.glk_window_get_root().unwrap();
    assert_eq!(root.as_ptr(), root_pointer);
}

#[test]
fn missing_file_restore_leaves_live_state_untouched() {
    let _guard = TEST_LOCK.lock().unwrap();
    let mut api = test_api();
    let root = api
        .glk_window_open(None, 0, 0, WindowType::Buffer, 1)
        .unwrap();
    let root_pointer = root.as_ptr();
    drop(root);
    api.system
        .files
        .insert("/story".into(), b"abcd".to_vec().into_boxed_slice());
    let fileref = api.glkunix_fileref_create_by_name_uncleaned(fileusage_Data, "/story".into(), 0);
    let stream = api
        .glk_stream_open_file(&lock!(fileref), FileMode::Read, 0)
        .unwrap()
        .unwrap();
    drop(stream);
    drop(fileref);
    let json = api.save_library_state(None, None).unwrap();
    api.system.files.remove("/story");
    let previous_event = api.glkunix_get_last_event_type();

    let mut state = LibraryState::from_json(&json).unwrap();
    assert!(matches!(
        api.update_from_library_state(&mut state),
        Err(AutosaveError::FileUnavailable(_))
    ));
    assert_eq!(api.glk_window_get_root().unwrap().as_ptr(), root_pointer);
    assert_eq!(api.glkunix_get_last_event_type(), previous_event);
}

#[test]
fn array_failure_leaves_live_state_untouched() {
    let _guard = TEST_LOCK.lock().unwrap();
    let mut api = test_api();
    let root = api
        .glk_window_open(None, 0, 0, WindowType::Buffer, 1)
        .unwrap();
    api.glk_request_line_event_uni(&mut lock!(root), vec![0; 8].into_boxed_slice(), 0)
        .unwrap();
    let root_pointer = root.as_ptr();
    drop(root);
    let json = api.save_library_state(None, None).unwrap();
    api.restore_array_callbacks_u32 = Some(restore_fail);
    let previous_event = api.glkunix_get_last_event_type();

    let mut state = LibraryState::from_json(&json).unwrap();
    assert!(matches!(
        api.update_from_library_state(&mut state),
        Err(AutosaveError::ArrayRestoreFailed)
    ));
    assert_eq!(api.glk_window_get_root().unwrap().as_ptr(), root_pointer);
    assert_eq!(api.glkunix_get_last_event_type(), previous_event);
}

#[test]
fn file_and_resource_streams_reopen_from_current_storage() {
    let _guard = TEST_LOCK.lock().unwrap();
    let mut api = test_api();
    let blorb = blorb_with_data(7, b"resource");
    assert_eq!(blorb.len(), 52);
    api.system.files.insert("/blorb".into(), blorb);
    let fileref = api.glkunix_fileref_create_by_name_uncleaned(fileusage_Data, "/blorb".into(), 0);
    let gamefile = api
        .glk_stream_open_file(&lock!(fileref), FileMode::Read, 0)
        .unwrap()
        .unwrap();
    api.giblorb_set_resource_map(gamefile.clone()).unwrap();
    let gamefile_tag = lock!(gamefile).id;
    drop(gamefile);
    drop(fileref);

    let resource = api.glk_stream_open_resource(7, 0).unwrap().unwrap();
    let resource_tag = lock!(resource).id;
    GlkApi::<TestSystem>::glk_stream_set_position(&mut lock!(resource), 3, SeekMode::Start);
    drop(resource);
    let json = api.save_library_state(None, None).unwrap();
    let mut state = LibraryState::from_json(&json).unwrap();
    api.update_from_library_state(&mut state).unwrap();

    let resource = api.streams.get_by_id(resource_tag).unwrap();
    assert_eq!(
        GlkApi::<TestSystem>::glk_stream_get_position(&mut lock!(resource)),
        3
    );
    let mut buf = [0; 4];
    assert_eq!(
        GlkApi::<TestSystem>::glk_get_buffer_stream(&mut lock!(resource), &mut buf).unwrap(),
        4
    );
    assert_eq!(&buf, b"ourc");
    drop(resource);

    assert!(api.giblorb_get_resource_map().is_some());
    let gamefile = api.streams.get_by_id(gamefile_tag).unwrap();
    api.giblorb_unset_resource_map().unwrap();
    api.giblorb_set_resource_map(gamefile.clone()).unwrap();
    drop(gamefile);
    let json = api.save_library_state(None, None).unwrap();
    api.system
        .files
        .insert("/blorb".into(), blorb_with_data(7, b"changed!"));
    api.giblorb_unset_resource_map().unwrap();
    let fileref = api.glkunix_fileref_create_by_name_uncleaned(fileusage_Data, "/blorb".into(), 0);
    let changed_gamefile = api
        .glk_stream_open_file(&lock!(fileref), FileMode::Read, 0)
        .unwrap()
        .unwrap();
    api.giblorb_set_resource_map(changed_gamefile.clone())
        .unwrap();
    drop(changed_gamefile);
    drop(fileref);
    let mut state = LibraryState::from_json(&json).unwrap();
    assert!(matches!(
        api.update_from_library_state(&mut state),
        Err(AutosaveError::ResourceChanged(7))
    ));
}

#[test]
fn nested_pair_grid_and_blank_windows_round_trip() {
    let _guard = TEST_LOCK.lock().unwrap();
    let mut api = test_api();
    let buffer = api
        .glk_window_open(None, 0, 0, WindowType::Buffer, 1)
        .unwrap();
    let grid = api
        .glk_window_open(
            Some(&buffer),
            winmethod_Right | winmethod_Fixed,
            10,
            WindowType::Grid,
            2,
        )
        .unwrap();
    drop(grid);
    let blank = api
        .glk_window_open(
            Some(&buffer),
            winmethod_Above | winmethod_Proportional,
            30,
            WindowType::Blank,
            3,
        )
        .unwrap();
    drop(blank);
    drop(buffer);
    assert!(matches!(
        GlkApi::<TestSystem>::glk_window_get_type(&lock!(api.glk_window_get_root().unwrap())),
        WindowType::Pair
    ));

    let json = api.save_library_state(None, None).unwrap();
    let mut state = LibraryState::from_json(&json).unwrap();
    api.update_from_library_state(&mut state).unwrap();
    assert_eq!(api.windows.iter().count(), 5);
    assert_eq!(api.streams.iter().count(), 5);
    assert!(matches!(
        GlkApi::<TestSystem>::glk_window_get_type(&lock!(api.glk_window_get_root().unwrap())),
        WindowType::Pair
    ));
    let second_json = api.save_library_state(None, None).unwrap();
    LibraryState::from_json(&second_json).unwrap();

    let Update::State(update) = api.update() else {
        panic!("expected state update")
    };
    assert_eq!(update.windows.len(), 2);
}
