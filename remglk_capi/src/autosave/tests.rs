use super::*;

extern "C" fn serialize_param(context: *mut SerializeContext, rock: *mut c_void) -> c_int {
    unsafe { glkunix_serialize_uint32(context, c"param".as_ptr(), rock.cast::<u32>().read()) };
    1
}

extern "C" fn serialize_root(context: *mut SerializeContext, _: *mut c_void) -> c_int {
    unsafe {
        glkunix_serialize_uint32(context, c"glulx_extra_state".as_ptr(), 1);
        glkunix_serialize_object(
            context,
            c"nested".as_ptr(),
            Some(serialize_param),
            (&mut 42u32 as *mut u32).cast(),
        );
        let mut values = [7u32, 9];
        glkunix_serialize_object_list(
            context,
            c"items".as_ptr(),
            Some(serialize_param),
            values.len() as c_int,
            size_of::<u32>(),
            values.as_mut_ptr().cast(),
        );
    }
    1
}

extern "C" fn unserialize_param(context: *mut UnserializeContext, rock: *mut c_void) -> c_int {
    unsafe {
        let mut value = 0;
        if glkunix_unserialize_uint32(context, c"param".as_ptr(), &mut value) == 0 {
            return 0;
        }
        rock.cast::<u32>().write(value);
    }
    1
}

#[test]
fn object_and_list_callbacks_round_trip_glulxe_extra_state() {
    let mut context = SerializeContext::new();
    assert_eq!(serialize_root(&mut context, ptr::null_mut()), 1);
    assert!(!context.failed);
    let value = Value::Object(context.value);
    assert_eq!(value["glulx_extra_state"], 1);
    assert_eq!(value["nested"]["param"], 42);
    assert_eq!(value["items"][0]["param"], 7);

    let mut root = UnserializeContext::new(value);
    unsafe {
        let mut nested = ptr::null_mut();
        assert_eq!(
            glkunix_unserialize_struct(&mut root, c"nested".as_ptr(), &mut nested),
            1
        );
        let mut value = 0;
        assert_eq!(
            glkunix_unserialize_uint32(nested, c"param".as_ptr(), &mut value),
            1
        );
        assert_eq!(value, 42);

        let mut list = ptr::null_mut();
        let mut count = 0;
        assert_eq!(
            glkunix_unserialize_list(&mut root, c"items".as_ptr(), &mut list, &mut count),
            1
        );
        assert_eq!(count, 2);
        let mut entry = ptr::null_mut();
        assert_eq!(glkunix_unserialize_list_entry(list, 1, &mut entry), 1);
        assert_eq!(
            glkunix_unserialize_uint32(entry, c"param".as_ptr(), &mut value),
            1
        );
        assert_eq!(value, 9);

        let mut output = [0u32; 2];
        assert_eq!(
            glkunix_unserialize_object_list_entries(
                list,
                Some(unserialize_param),
                count,
                size_of::<u32>(),
                output.as_mut_ptr().cast()
            ),
            1
        );
        assert_eq!(output, [7, 9]);
        assert_eq!(glkunix_unserialize_list_entry(list, 2, &mut entry), 0);
        assert_eq!(
            glkunix_unserialize_uint32(entry, c"missing".as_ptr(), &mut value),
            0
        );
        root.free_children();
    }
}

extern "C" fn failing_callback(_: *mut SerializeContext, _: *mut c_void) -> c_int {
    0
}

#[test]
fn callback_failures_and_invalid_counts_are_not_accepted() {
    assert!(serialize_extra_state(Some(failing_callback), ptr::null_mut()).is_err());
    let mut context = SerializeContext::new();
    unsafe {
        glkunix_serialize_object_list(
            &mut context,
            c"items".as_ptr(),
            Some(serialize_param),
            -1,
            size_of::<u32>(),
            ptr::null_mut(),
        );
    }
    assert!(context.failed);

    let mut root =
        UnserializeContext::new(serde_json::json!({"value": "not an integer", "list": []}));
    unsafe {
        let mut value = 0;
        assert_eq!(
            glkunix_unserialize_uint32(&mut root, c"value".as_ptr(), &mut value),
            0
        );
        assert_eq!(
            glkunix_unserialize_object_list_entries(
                &mut root,
                Some(unserialize_param),
                1,
                size_of::<u32>(),
                ptr::null_mut()
            ),
            0
        );
    }
}
