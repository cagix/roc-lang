//! Windows-specific implementation of gather I/O using WriteFileGather.

const std = @import("std");
const windows = std.os.windows;
const WINAPI = windows.WINAPI;
const HANDLE = windows.HANDLE;
const OVERLAPPED = windows.OVERLAPPED;
const DWORD = windows.DWORD;

const write_gather = @import("write_gather.zig");
const WriteGatherError = write_gather.WriteGatherError;
const BufferVec = write_gather.BufferVec;

/// Gets the sector size for a given file handle. WriteFileGather
/// requires buffer pointers to be both aligned to the sector size, and also have sizes
/// that are multiples of the sector size.
/// 
/// Note: This function never fails - it returns a default sector size (512 bytes) if 
/// the actual sector size cannot be determined.
pub fn getSectorSize(file_handle: std.fs.File) usize {
    var bytes_per_sector: DWORD = undefined;
    var sectors_per_cluster: DWORD = undefined;
    var number_of_free_clusters: DWORD = undefined;
    var total_number_of_clusters: DWORD = undefined;

    // Get volume information to determine sector size
    if (windows.kernel32.GetDiskFreeSpaceW(
        file_handle.handle,
        &sectors_per_cluster,
        &bytes_per_sector,
        &number_of_free_clusters,
        &total_number_of_clusters,
    ) == 0) {
        // Handle couldn't get the sector size directly, try fallback approach
        // Default to 512 bytes, which is the minimum sector size on most disks
        return 512;
    }

    return @intCast(bytes_per_sector);
}

/// Validates that buffers meet the Windows WriteFileGather requirements:
/// - Each buffer must be aligned to the sector size
/// - Each buffer's size must be a multiple of the sector size
fn validateBuffers(file_handle: std.fs.File, buffers: []const BufferVec) !void {
    const sector_size = getSectorSize(file_handle);
    const sector_size_mask = sector_size - 1;

    for (buffers) |buffer| {
        // Check buffer alignment
        const ptr_addr = @intFromPtr(buffer.ptr);
        if ((ptr_addr & sector_size_mask) != 0) {
            return WriteGatherError.InvalidBufferAlignment;
        }

        // Check buffer size is multiple of sector size
        if ((buffer.len & sector_size_mask) != 0) {
            return WriteGatherError.InvalidBufferSize;
        }
    }
}

/// Implementation of writeGather for Windows using WriteFileGather
pub fn writeGather(
    file_handle: std.fs.File,
    buffers: []const BufferVec,
    offset: u64,
) WriteGatherError!usize {
    // Only validate buffers in debug builds for better performance in release builds
    if (std.debug.runtime_safety) {
        try validateBuffers(file_handle, buffers);
    }

    // Prepare OVERLAPPED structure with the file offset
    var overlapped = std.mem.zeroes(OVERLAPPED);
    overlapped.Internal = 0;
    overlapped.InternalHigh = 0;
    overlapped.Offset = @truncate(offset & 0xFFFFFFFF);
    overlapped.OffsetHigh = @truncate(offset >> 32);
    overlapped.hEvent = windows.kernel32.CreateEventW(null, windows.TRUE, windows.FALSE, null);
    defer _ = windows.kernel32.CloseHandle(overlapped.hEvent);

    // Create array of pointers for WriteFileGather
    var buffer_ptrs = try std.heap.page_allocator.alloc([*]const u8, buffers.len);
    defer std.heap.page_allocator.free(buffer_ptrs);

    var total_size: usize = 0;

    // Fill buffer pointers array and calculate total size
    for (buffers, 0..) |buffer, i| {
        buffer_ptrs[i] = buffer.ptr;
        total_size += buffer.len;
    }

    // Perform the gather write operation
    const success = windows.kernel32.WriteFileGather(
        file_handle.handle,
        @ptrCast(&buffer_ptrs[0]),
        @intCast(total_size),
        null, // Must be null for WriteFileGather
        &overlapped,
    );

    // Check for immediate completion
    if (success == 0) {
        const err = windows.kernel32.GetLastError();

        // If operation is pending, wait for completion
        if (err == windows.ERROR.IO_PENDING) {
            var bytes_transferred: DWORD = undefined;
            const wait_success = windows.kernel32.GetOverlappedResult(
                file_handle.handle,
                &overlapped,
                &bytes_transferred,
                windows.TRUE, // Wait for completion
            );

            return if (wait_success == 0)
                translateError(windows.kernel32.GetLastError())
            else
                @intCast(bytes_transferred);
        } else {
            return translateError(err);
        }
    }

    return total_size;
}

/// Translates Windows-specific errors to WriteGatherError
fn translateError(err: windows.Win32Error) WriteGatherError {
    return switch (err) {
        windows.ERROR.FILE_NOT_FOUND => WriteGatherError.FileNotFound,
        windows.ERROR.PATH_NOT_FOUND => WriteGatherError.BadPathName,
        windows.ERROR.ACCESS_DENIED => WriteGatherError.AccessDenied,
        windows.ERROR.INVALID_HANDLE => WriteGatherError.InvalidHandle,
        windows.ERROR.INVALID_PARAMETER => WriteGatherError.InvalidArgument,
        windows.ERROR.NOT_ENOUGH_MEMORY => WriteGatherError.OutOfMemory,
        windows.ERROR.OPERATION_ABORTED => WriteGatherError.OperationAborted,
        windows.ERROR.NOT_SUPPORTED => WriteGatherError.InvalidArgument,
        windows.ERROR.INVALID_ACCESS => WriteGatherError.AccessDenied,
        windows.ERROR.BROKEN_PIPE => WriteGatherError.ConnectionResetByPeer,
        windows.ERROR.DISK_FULL => WriteGatherError.InputOutput,
        windows.ERROR.IO_DEVICE => WriteGatherError.InputOutput,
        else => WriteGatherError.Unexpected,
    };
}