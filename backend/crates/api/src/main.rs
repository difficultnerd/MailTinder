#[cfg(not(feature = "testkit"))]
fn main() {}

#[cfg(feature = "testkit")]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    api::test_runtime::serve().await
}
