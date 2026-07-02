fn main() -> Result<(), Box<dyn std::error::Error>> {
    build_utils::include_artifacts("lee/privacy_preserving_circuit")?;

    Ok(())
}
