use chrono::Local;

pub fn now() -> String {
    Local::now().format("%H:%M:%S%.3f").to_string()
}

#[macro_export]
macro_rules! tlog {
    ($tag:expr, $($arg:tt)*) => {
        println!("{} [{}] {}", $crate::log::now(), $tag, format!($($arg)*))
    };
}
