pub mod clips;
pub mod config;
pub mod library;
pub mod store;
pub mod timeline;

pub fn greet(name: &str) -> String {
    let trimmed = name.trim();
    let target = if trimmed.is_empty() { "world" } else { trimmed };
    format!("Hello, {target}!")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn greets_name() {
        assert_eq!(greet("SUBCULT"), "Hello, SUBCULT!");
    }
}
