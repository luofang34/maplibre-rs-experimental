use super::*;

static LOGGING: Once = Once::new();

/// The system allocator with a running count of live bytes, so the renderer can charge a
/// frame's heap growth to the system it happens in.
struct CountingAllocator;

// SAFETY: every call forwards to the system allocator unchanged; only a counter is kept.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            heap::account(layout.size() as isize);
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        heap::account(-(layout.size() as isize));
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let moved = unsafe { System.realloc(pointer, layout, new_size) };
        if !moved.is_null() {
            heap::account(new_size as isize - layout.size() as isize);
        }
        moved
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

/// Name of the log file written next to the tile cache; a console cannot always be
/// attached to the device, and an abort leaves nothing else behind.
const LOG_FILE: &str = "maplibre.log";
/// Name of the file a panic is appended to before the process aborts.
const PANIC_FILE: &str = "panic.log";

/// Writes every log line to standard output and, when a directory is given, to a file in it.
/// Neither write may fail: the subscriber reports a failed write on standard error, and the
/// process aborts when that fails too, which it does once the console a launcher attached
/// has gone away. Standard output and error are best effort here.
struct Tee(Option<Arc<Mutex<File>>>);

struct TeeWriter(Option<Arc<Mutex<File>>>);

impl<'a> MakeWriter<'a> for Tee {
    type Writer = TeeWriter;

    fn make_writer(&'a self) -> TeeWriter {
        TeeWriter(self.0.clone())
    }
}

impl Write for TeeWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        std::io::stdout().write_all(buf).ok();
        if let Some(file) = &self.0 {
            if let Ok(mut file) = file.lock() {
                file.write_all(buf).ok();
                file.flush().ok();
            }
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        std::io::stdout().flush().ok();
        Ok(())
    }
}

pub(super) fn init_logging(directory: Option<&str>) {
    LOGGING.call_once(|| {
        let file = directory
            .and_then(|directory| File::create(Path::new(directory).join(LOG_FILE)).ok())
            .map(|file| Arc::new(Mutex::new(file)));
        // wgpu reports every wait on a submission at INFO, ten lines a frame.
        let targets = Targets::new()
            .with_default(tracing::Level::INFO)
            .with_target("wgpu_core", tracing::Level::WARN)
            .with_target("wgpu_hal", tracing::Level::WARN)
            .with_target("naga", tracing::Level::WARN);
        tracing_subscriber::registry()
            .with(
                tracing_subscriber::fmt::layer()
                    .with_ansi(false)
                    .with_writer(Tee(file))
                    .with_filter(targets),
            )
            .init();
        if let Some(directory) = directory {
            let panic_path = Path::new(directory).join(PANIC_FILE);
            std::panic::set_hook(Box::new(move |info| {
                let backtrace = std::backtrace::Backtrace::force_capture();
                tracing::error!(%info, "panic");
                if let Ok(mut file) = OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&panic_path)
                {
                    file.write_all(format!("{info}\n{backtrace}\n").as_bytes())
                        .ok();
                }
            }));
        }
    });
}
