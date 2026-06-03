// GPS time sync bridge: feed the system clock discipline daemon (chrony) with
// UTC time parsed from the receiver's RMC stream, via the standard NTP shared
// memory (SHM) refclock protocol. This is the same SHM interface gpsd uses.
//
// The logger owns the receiver's serial port exclusively, so gpsd cannot read
// it; instead the logger — which already sees every NMEA sentence — writes time
// samples to SHM unit 0 and chrony (`refclock SHM 0`) does the actual clock
// discipline. Everything here is best-effort: failures are reported once and
// never interrupt logging.

use crate::shared::nmea::{NmeaSentenceCollector, parse_rmc_utc};
use chrono::{DateTime, Utc};
use std::io;
use std::ptr;
use std::sync::atomic::{Ordering, fence};
use std::time::{SystemTime, UNIX_EPOCH};

// NTP SHM segments are keyed `0x4e545030 + unit` ("NTP0", "NTP1", ...).
const NTP_SHM_BASE_KEY: libc::key_t = 0x4e54_5030;

// Mirror of ntpd/gpsd's `struct shmTime`. `#[repr(C)]` reproduces the same field
// order, alignment, and padding the C definition has on LP64 Linux, so chrony
// reads the fields at the offsets it expects.
#[repr(C)]
struct ShmTime {
    mode: libc::c_int,
    count: libc::c_int,
    clock_time_stamp_sec: libc::time_t,
    clock_time_stamp_usec: libc::c_int,
    receive_time_stamp_sec: libc::time_t,
    receive_time_stamp_usec: libc::c_int,
    leap: libc::c_int,
    precision: libc::c_int,
    nsamples: libc::c_int,
    valid: libc::c_int,
    clock_time_stamp_nsec: libc::c_uint,
    receive_time_stamp_nsec: libc::c_uint,
    dummy: [libc::c_int; 8],
}

// Writes time samples into one NTP SHM segment using the lock-free, count-based
// handshake (mode 1) that ntpd and chrony understand.
struct ShmTimeWriter {
    shm: *mut ShmTime,
}

impl ShmTimeWriter {
    fn attach(unit: i32) -> io::Result<Self> {
        let key = NTP_SHM_BASE_KEY + unit as libc::key_t;
        let size = std::mem::size_of::<ShmTime>();
        // Mode 0666 so chrony (running as _chrony after dropping privileges) can
        // attach the root-created segment. Acceptable on a dedicated appliance.
        let flags = libc::IPC_CREAT | 0o666;

        // SAFETY: standard System V shared memory attach. We created/opened the
        // segment for our own process and keep the returned pointer for the
        // lifetime of the writer; it is detached in Drop.
        unsafe {
            let shmid = libc::shmget(key, size, flags);
            if shmid == -1 {
                return Err(io::Error::last_os_error());
            }
            let addr = libc::shmat(shmid, ptr::null(), 0);
            if addr == usize::MAX as *mut libc::c_void {
                return Err(io::Error::last_os_error());
            }
            let shm = addr as *mut ShmTime;
            // Select the count-based handshake protocol.
            (*shm).mode = 1;
            Ok(Self { shm })
        }
    }

    fn write_sample(&self, clock: DateTime<Utc>, received: SystemTime) {
        let clock_sec = clock.timestamp();
        let clock_nsec = clock.timestamp_subsec_nanos();
        let recv = received.duration_since(UNIX_EPOCH).unwrap_or_default();
        let recv_sec = recv.as_secs() as libc::time_t;
        let recv_nsec = recv.subsec_nanos();

        // SAFETY: `shm` points at our attached segment for the writer's lifetime.
        // The count/valid handshake with full fences signals a consistent sample
        // to the reader: bump count to odd, write fields, bump count to even.
        unsafe {
            let shm = self.shm;
            ptr::write_volatile(&mut (*shm).valid, 0);
            ptr::write_volatile(&mut (*shm).count, (*shm).count.wrapping_add(1));
            fence(Ordering::SeqCst);

            (*shm).clock_time_stamp_sec = clock_sec as libc::time_t;
            (*shm).clock_time_stamp_usec = (clock_nsec / 1_000) as libc::c_int;
            (*shm).clock_time_stamp_nsec = clock_nsec;
            (*shm).receive_time_stamp_sec = recv_sec;
            (*shm).receive_time_stamp_usec = (recv_nsec / 1_000) as libc::c_int;
            (*shm).receive_time_stamp_nsec = recv_nsec;
            (*shm).leap = 0; // LEAP_NOWARNING
            (*shm).precision = -5; // ~31 ms; informational, chrony can override

            fence(Ordering::SeqCst);
            ptr::write_volatile(&mut (*shm).count, (*shm).count.wrapping_add(1));
            ptr::write_volatile(&mut (*shm).valid, 1);
        }
    }
}

impl Drop for ShmTimeWriter {
    fn drop(&mut self) {
        // SAFETY: detaching our own valid attachment; the segment itself is left
        // in place so chrony keeps reading and the next run reattaches it.
        unsafe {
            libc::shmdt(self.shm as *const libc::c_void);
        }
    }
}

// Frames NMEA sentences out of the raw serial byte stream and writes a time
// sample to SHM for every valid RMC fix. Lives on the logging thread and is fed
// the same bytes the logger already reads; it never blocks or fails logging.
pub struct GpsClockBridge {
    collector: NmeaSentenceCollector,
    writer: ShmTimeWriter,
}

impl GpsClockBridge {
    // Attach NTP SHM unit 0. Returns an error only if the segment cannot be
    // created/attached (e.g. insufficient privileges); the caller treats that as
    // "time sync unavailable" and continues logging normally.
    pub fn new() -> io::Result<Self> {
        Ok(Self {
            collector: NmeaSentenceCollector::new(),
            writer: ShmTimeWriter::attach(0)?,
        })
    }

    pub fn ingest(&mut self, bytes: &[u8]) {
        let mut sentences = Vec::new();
        self.collector.push_bytes(bytes, &mut sentences);
        for sentence in sentences {
            if let Some(utc) = parse_rmc_utc(&sentence) {
                self.writer.write_sample(utc, SystemTime::now());
            }
        }
    }
}
