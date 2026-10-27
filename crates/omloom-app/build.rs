fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_prost_build::configure()
        .build_client(false)
        .extern_path(".omloom.v1", "::omloom_contracts::wire")
        .extern_path(".omloom.workspace.v1", "::omloom_contracts::workspace")
        .extern_path(".omloom.workspace.v1.Request", "crate::codec::Received")
        .codec_path("crate::codec::WorkspaceCodec")
        .compile_protos(&["../../proto/omloom/v1/workspace.proto"], &["../../proto"])?;
    Ok(())
}
