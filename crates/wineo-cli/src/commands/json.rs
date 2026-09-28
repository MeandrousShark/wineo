use serde::Serialize;

pub fn print(value: &impl Serialize) -> anyhow::Result<()> {
    println!("{}", serde_json::to_string(value)?);
    Ok(())
}
