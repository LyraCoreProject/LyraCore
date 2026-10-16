// Both sides use the 2.7.1 native pointer ABI; these identifiers are private to this experiment.
macro_rules! native_syscalls {
    ($declare:ident) => {
        $declare! {
            0 => table_id_from_name(name: *const u8, len: usize, out: *mut u32) -> u16;
            1 => index_id_from_name(name: *const u8, len: usize, out: *mut u32) -> u16;
            2 => datastore_table_row_count(table: u32, out: *mut u64) -> u16;
            3 => datastore_table_scan_bsatn(table: u32, out: *mut u32) -> u16;
            4 => datastore_index_scan_range_bsatn(index: u32, prefix: *const u8, prefix_len: usize, count: u16, start: *const u8, start_len: usize, end: *const u8, end_len: usize, out: *mut u32) -> u16;
            4 => datastore_btree_scan_bsatn(index: u32, prefix: *const u8, prefix_len: usize, count: u16, start: *const u8, start_len: usize, end: *const u8, end_len: usize, out: *mut u32) -> u16;
            5 => datastore_delete_by_index_scan_range_bsatn(index: u32, prefix: *const u8, prefix_len: usize, count: u16, start: *const u8, start_len: usize, end: *const u8, end_len: usize, out: *mut u32) -> u16;
            5 => datastore_delete_by_btree_scan_bsatn(index: u32, prefix: *const u8, prefix_len: usize, count: u16, start: *const u8, start_len: usize, end: *const u8, end_len: usize, out: *mut u32) -> u16;
            6 => datastore_index_scan_point_bsatn(index: u32, point: *const u8, len: usize, out: *mut u32) -> u16;
            7 => datastore_delete_by_index_scan_point_bsatn(index: u32, point: *const u8, len: usize, out: *mut u32) -> u16;
            8 => row_iter_bsatn_advance(iter: u32, buffer: *mut u8, len: *mut usize) -> i16;
            9 => row_iter_bsatn_close(iter: u32) -> u16;
            10 => datastore_insert_bsatn(table: u32, row: *mut u8, len: *mut usize) -> u16;
            11 => datastore_update_bsatn(table: u32, index: u32, row: *mut u8, len: *mut usize) -> u16;
            12 => datastore_delete_all_by_eq_bsatn(table: u32, rows: *const u8, len: usize, out: *mut u32) -> u16;
            13 => datastore_clear(table: u32, out: *mut u64) -> u16;
            14 => console_log(level: u8, target: *const u8, target_len: usize, filename: *const u8, filename_len: usize, line: u32, message: *const u8, len: usize) -> ();
            15 => identity(out: *mut u8) -> ();
            16 => bytes_sink_write(id: u32, buffer: *const u8, len: *mut usize) -> u16;
            17 => bytes_source_read(id: u32, buffer: *mut u8, len: *mut usize) -> i16;
            18 => bytes_source_remaining_length(id: u32, out: *mut u32) -> i16;
            19 => get_jwt(connection: *const u8, source: *mut u32) -> u16;
            20 => console_timer_start(name: *const u8, len: usize) -> u32;
            21 => console_timer_end(id: u32) -> u16;
            22 => volatile_nonatomic_schedule_immediate(name: *const u8, name_len: usize, args: *const u8, args_len: usize) -> ();
            23 => procedure_sleep_until(at: i64) -> i64;
            24 => procedure_start_mut_tx(at: *mut i64) -> u16;
            25 => procedure_commit_mut_tx() -> u16;
            26 => procedure_abort_mut_tx() -> u16;
            27 => procedure_http_request(req: *const u8, req_len: u32, body: *const u8, body_len: u32, response: *mut [u32; 2]) -> u16;
        }
    };
}

type Dispatch = unsafe extern "C" fn(*mut std::ffi::c_void, u32, *const usize, usize) -> i64;
