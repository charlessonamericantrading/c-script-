//! Fase 0 validation tool: parse a real GGUF file and dump everything this
//! crate knows about it, so its output can be eyeballed against a trusted
//! reference (`ollama show --modelfile`, `gguf-py`) tensor by tensor.

use std::env;
use std::fs;
use std::process::ExitCode;

use gguf::GgufFile;

fn main() -> ExitCode {
    let path = match env::args().nth(1) {
        Some(p) => p,
        None => {
            eprintln!("usage: gguf-inspect <path-to-model.gguf>");
            return ExitCode::FAILURE;
        }
    };

    let bytes = match fs::read(&path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("error: could not read {path}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let file_len = bytes.len() as u64;

    let gguf = match GgufFile::parse(&bytes) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("error: failed to parse {path}: {e}");
            return ExitCode::FAILURE;
        }
    };

    println!("file             {path}");
    println!("file size        {file_len} bytes ({:.2} MiB)", file_len as f64 / (1024.0 * 1024.0));
    println!("gguf version     {}", gguf.header.version);
    println!("tensor count     {}", gguf.header.tensor_count);
    println!("metadata kv      {}", gguf.header.metadata_kv_count);
    println!("alignment        {}", gguf.alignment);
    println!("tensor data @    {}", gguf.tensor_data_offset);
    println!();

    println!("-- metadata (arrays elided past 3 entries) --");
    for (k, v) in gguf.metadata_in_order() {
        println!("  {k:<40} {}", v.preview());
    }
    println!();

    println!("-- tensors --");
    println!(
        "  {:<40} {:<22} {:<10} {:>14} {:>14} {:>14}",
        "name", "shape", "type", "rel.offset", "abs.offset", "size (bytes)"
    );

    let mut unknown_type_count = 0usize;
    let mut unsized_count = 0usize;
    let mut max_end: u64 = gguf.tensor_data_offset;

    for t in &gguf.tensors {
        let shape = format!(
            "[{}]",
            t.dimensions.iter().map(u64::to_string).collect::<Vec<_>>().join(",")
        );
        let type_name = t.ggml_type.map(|ty| ty.name()).unwrap_or_else(|| {
            unknown_type_count += 1;
            "?unknown"
        });

        let abs_offset = match gguf.tensor_absolute_offset(t) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("  {:<40} ERROR computing absolute offset: {e}", t.name);
                continue;
            }
        };

        let size = t.size_bytes();
        let size_str = match size {
            Some(s) => {
                max_end = max_end.max(abs_offset + s);
                s.to_string()
            }
            None => {
                unsized_count += 1;
                "?".to_string()
            }
        };

        println!(
            "  {:<40} {:<22} {:<10} {:>14} {:>14} {:>14}",
            t.name, shape, type_name, t.offset, abs_offset, size_str
        );
    }

    println!();
    println!("-- sanity check --");
    println!("  architecture           {}", gguf.architecture().unwrap_or("(none)"));
    println!("  tensors w/ unknown type   {unknown_type_count}");
    println!("  tensors w/ unsized type   {unsized_count}");
    println!("  computed max end offset  {max_end}");
    println!("  actual file size         {file_len}");

    if unknown_type_count > 0 || unsized_count > 0 {
        println!("  status                   PARTIAL — some tensor types unsupported (see counts above)");
    } else if max_end > file_len {
        println!("  status                   FAIL — computed tensor data runs past end of file");
        return ExitCode::FAILURE;
    } else {
        println!("  status                   OK — every tensor's computed extent fits inside the file");
    }

    ExitCode::SUCCESS
}
