//! URL slug helper for a small blog engine.

/// Turn a human title into a URL slug.
///
/// e.g. `slugify("My First Post!")` should be `"my-first-post"`.
pub fn slugify(_title: &str) -> String {
    // TODO: implement
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_title() {
        assert_eq!(slugify("My First Post!"), "my-first-post");
    }

    #[test]
    fn two_words() {
        assert_eq!(slugify("Hello World"), "hello-world");
    }
}
