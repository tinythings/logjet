use std::ffi::c_char;
use std::mem::{align_of, size_of};

use super::*;

#[test]
fn string_and_bytes_stay_pointer_len_pairs() {
    assert_eq!(size_of::<LjxAbiString>(), size_of::<(*const c_char, usize)>());
    assert_eq!(align_of::<LjxAbiString>(), align_of::<(*const c_char, usize)>());
    assert_eq!(size_of::<LjxAbiBytes>(), size_of::<(*const u8, usize)>());
    assert_eq!(align_of::<LjxAbiBytes>(), align_of::<(*const u8, usize)>());
}

#[test]
fn exporter_ctx_stays_opaque_zero_sized() {
    assert_eq!(size_of::<LjxExporterCtx>(), 0);
    assert_eq!(align_of::<LjxExporterCtx>(), 1);
}

#[test]
fn v1_defaults_report_current_struct_sizes_and_zero_reserved_fields() {
    let host = LjxExportHostV1::default();
    let init = LjxExportInitV1::default();
    let descriptor = LjxExporterDescriptorV1::header();

    assert_eq!(host.struct_size, size_of::<LjxExportHostV1>() as u32);
    assert_eq!(host.reserved, [0; 6]);
    assert_eq!(init.struct_size, size_of::<LjxExportInitV1>() as u32);
    assert_eq!(init.reserved, [0; 4]);
    assert_eq!(descriptor.struct_size, size_of::<LjxExporterDescriptorV1>() as u32);
    assert_eq!(descriptor.reserved, [0; 6]);
    assert_eq!(descriptor.abi_major, LJX_EXPORTER_ABI_MAJOR);
    assert_eq!(descriptor.abi_minor, LJX_EXPORTER_ABI_MINOR);
}

#[test]
fn function_pointer_slots_stay_thin_c_indirections() {
    let descriptor = LjxExporterDescriptorV1::header();
    let _create: unsafe extern "C" fn(*const LjxExportHostV1, *const LjxExportInitV1) -> *mut LjxExporterCtx = descriptor.create;
    let _write: unsafe extern "C" fn(*mut LjxExporterCtx, *const LjxExportRecordV1) -> i32 = descriptor.write_record;
    let _finish: unsafe extern "C" fn(*mut LjxExporterCtx) -> i32 = descriptor.finish;
    let _last_error: unsafe extern "C" fn(*mut LjxExporterCtx) -> LjxAbiString = descriptor.last_error;
    let _free: unsafe extern "C" fn(*mut LjxExporterCtx) = descriptor.free;
}
