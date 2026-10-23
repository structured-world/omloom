use prost::Message;
use prost_types::{
    FileDescriptorSet,
    field_descriptor_proto::{Label, Type},
};
use std::{env, error::Error, fs, path::PathBuf};

fn main() -> Result<(), Box<dyn Error>> {
    let proto = "../../proto/omloom/v1/contracts.proto";
    println!("cargo:rerun-if-changed={proto}");
    let out = PathBuf::from(env::var("OUT_DIR")?);
    let descriptor = out.join("contracts.bin");
    prost_build::Config::new()
        .file_descriptor_set_path(&descriptor)
        .compile_protos(&[proto], &["../../proto"])?;
    let set = FileDescriptorSet::decode(fs::read(descriptor)?.as_slice())?;
    let messages = &set.file[0].message_type;
    let mut shapes = String::from("const SHAPES: &[&[Shape]] = &[\n");
    for message in messages {
        shapes.push_str("&[\n");
        for field in &message.field {
            let kind = match field.r#type() {
                Type::Message => {
                    let name = field.type_name.as_deref().ok_or("missing type")?;
                    let index = messages
                        .iter()
                        .position(|m| name == format!(".omloom.v1.{}", m.name()))
                        .ok_or("unknown message")?;
                    format!("Kind::Message({index})")
                }
                Type::String => "Kind::Text".into(),
                Type::Bytes => "Kind::Bytes".into(),
                Type::Uint32 => "Kind::Varint(u32::MAX as u64)".into(),
                Type::Bool => "Kind::Varint(1)".into(),
                Type::Enum => "Kind::Varint(i32::MAX as u64)".into(),
                Type::Uint64 | Type::Sint64 => "Kind::Varint(u64::MAX)".into(),
                _ => return Err("add a preflight rule before using this protobuf type".into()),
            };
            shapes.push_str(&format!(
                "Shape {{ tag: {}, kind: {kind}, repeated: {}, group: {} }},\n",
                field.number(),
                field.label() == Label::Repeated,
                field.oneof_index.map_or(0, |index| index + 1)
            ));
        }
        shapes.push_str("],\n");
    }
    shapes.push_str("];");
    let record = messages
        .iter()
        .position(|m| m.name() == "Record")
        .ok_or("no Record")?;
    shapes.push_str(&format!("\nconst ROOT: usize = {record};\n"));
    for message in messages {
        // Fails compilation if schema growth invalidates the preflight envelope.
        shapes.push_str(&format!(
            "const _: () = assert!(std::mem::size_of::<crate::wire::{}>() <= 256);\n",
            message.name()
        ));
    }
    fs::write(out.join("shapes.rs"), shapes)?;
    Ok(())
}
