//! URL slug helper for a small blog engine.

/// Turn a human title into a URL slug.
///
/// e.g. `slugify("My First Post!")` should be `"my-first-post"`.
pub fn slugify(title: &str) -> String {
    let mut out = String::new();
    let mut pending_sep = false;
    for c in title.chars() {
        if c.is_ascii_alphanumeric() {
            if pending_sep && !out.is_empty() {
                out.push('-');
            }
            out.push(c.to_ascii_lowercase());
            pending_sep = false;
        } else {
            pending_sep = true;
        }
    }
    out
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
