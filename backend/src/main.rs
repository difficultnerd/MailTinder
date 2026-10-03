fn main() {
    println!("{}", greeting());
}

fn greeting() -> &'static str {
    "hello from backend"
}

#[cfg(test)]
mod tests {
    use super::greeting;

    #[test]
    fn greeting_is_stable() {
        assert_eq!(greeting(), "hello from backend");
    }
}
