use std::any::type_name;

pub struct SafeError<'a, T: ?Sized> {
    value: &'a T,
}

pub fn safe_error<T: ?Sized>(value: &T) -> SafeError<'_, T> {
    SafeError { value }
}

impl<T: ?Sized> std::fmt::Debug for SafeError<'_, T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let _ = self.value;
        formatter
            .debug_tuple("SafeError")
            .field(&type_name::<T>())
            .finish()
    }
}

impl<T: ?Sized> std::fmt::Display for SafeError<'_, T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let _ = self.value;
        formatter.write_str(type_name::<T>())
    }
}

#[cfg(test)]
mod tests {
    use super::safe_error;

    #[test]
    fn safe_error_keeps_only_the_error_type() {
        let error = std::io::Error::other("password=top-secret");
        let debug = format!("{:?}", safe_error(&error));
        let display = safe_error(&error).to_string();
        assert!(debug.contains("std::io::error::Error"));
        assert!(display.contains("std::io::error::Error"));
        assert!(!debug.contains("top-secret"));
        assert!(!display.contains("top-secret"));
    }
}
