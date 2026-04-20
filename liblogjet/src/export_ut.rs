use core::ffi::c_char;
use core::mem::{align_of, offset_of, size_of};

use super::*;

#[test]
fn abi_string_and_bytes_stay_plain_pointer_len_pairs() {
    assert_eq!(size_of::<LjxAbiString>(), size_of::<*const c_char>() + size_of::<usize>());
    assert_eq!(align_of::<LjxAbiString>(), align_of::<usize>());
    assert_eq!(offset_of!(LjxAbiString, ptr), 0);
    assert_eq!(offset_of!(LjxAbiString, len), size_of::<*const c_char>());

    assert_eq!(size_of::<LjxAbiBytes>(), size_of::<*const u8>() + size_of::<usize>());
    assert_eq!(align_of::<LjxAbiBytes>(), align_of::<usize>());
    assert_eq!(offset_of!(LjxAbiBytes, ptr), 0);
    assert_eq!(offset_of!(LjxAbiBytes, len), size_of::<*const u8>());
}

#[test]
fn v1_defaults_report_current_struct_sizes_and_zero_reserved_fields() {
    let init = LjxExportInitV1::default();
    assert_eq!(init.struct_size as usize, size_of::<LjxExportInitV1>());
    assert_eq!(init.flags, 0);
    assert!(init.options.is_null());
    assert_eq!(init.options_len, 0);
    assert_eq!(init.reserved, [0; 4]);

    let record = LjxExportRecordV1::default();
    assert_eq!(record.struct_size as usize, size_of::<LjxExportRecordV1>());
    assert_eq!(record.flags, 0);
    assert_eq!(record.record_type, LJX_RECORD_TYPE_LOGS);
    assert_eq!(record.payload_kind, LJX_PAYLOAD_KIND_OPAQUE);
    assert!(record.payload.ptr.is_null());
    assert_eq!(record.payload.len, 0);

    let host = LjxExportHostV1::default();
    assert_eq!(host.struct_size as usize, size_of::<LjxExportHostV1>());
    assert_eq!(host.flags, 0);
    assert!(host.user.is_null());
    assert!(host.flush.is_none());
    assert_eq!(host.reserved, [0; 6]);

    let descriptor = LjxExporterDescriptorV1::header();
    assert_eq!(descriptor.struct_size as usize, size_of::<LjxExporterDescriptorV1>());
    assert_eq!(descriptor.abi_major, LJX_EXPORTER_ABI_MAJOR);
    assert_eq!(descriptor.abi_minor, LJX_EXPORTER_ABI_MINOR);
    assert_eq!(descriptor.capabilities, 0);
    assert_eq!(descriptor.format_name, LjxAbiString::empty());
    assert_eq!(descriptor.display_name, LjxAbiString::empty());
    assert_eq!(descriptor.default_extension, LjxAbiString::empty());
    assert_eq!(descriptor.reserved, [0; 6]);
}

#[test]
fn function_pointer_slots_stay_thin_c_indirections() {
    assert_eq!(size_of::<LjxExporterCreateFn>(), size_of::<usize>());
    assert_eq!(size_of::<LjxExporterWriteRecordFn>(), size_of::<usize>());
    assert_eq!(size_of::<LjxExporterFinishFn>(), size_of::<usize>());
    assert_eq!(size_of::<LjxExporterLastErrorFn>(), size_of::<usize>());
    assert_eq!(size_of::<LjxExporterFreeFn>(), size_of::<usize>());
    assert_eq!(size_of::<Option<LjxExportFlushFn>>(), size_of::<usize>());
}

#[test]
fn opaque_exporter_context_stays_zero_sized() {
    assert_eq!(size_of::<LjxExporterCtx>(), 0);
    assert_eq!(align_of::<LjxExporterCtx>(), 1);
}
