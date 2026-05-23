use anyhow::{anyhow, Result};
use wasmtime::*;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct WasmDecryptor {
    engine: Engine,
    module: Module,
}

impl WasmDecryptor {
    pub fn new(wasm_bytes: &[u8]) -> Result<Self> {
        let engine = Engine::default();
        let module = Module::new(&engine, wasm_bytes)?;
        Ok(Self { engine, module })
    }

    pub fn decrypt(&self, hex_str: &str, tmdb_id: f64) -> Result<String> {
        let mut store = Store::new(&self.engine, ());

        let memory_ty = MemoryType::new(256, None); // 256 pages = 16MB
        let memory = Memory::new(&mut store, memory_ty)?;

        let seed_func = Func::wrap(&mut store, || -> f64 {
            let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
            let ms = now.as_secs_f64() * 1000.0 + (now.subsec_millis() as f64);
            // Math.random() approximation for the seed isn't strictly necessary to be truly random, 
            // since videasy just ignores it or uses it loosely. Let's just return a generic float.
            ms * 0.5
        });

        let abort_func = Func::wrap(&mut store, |_a: i32, _b: i32, _c: i32, _d: i32| {
            eprintln!("WASM abort called!");
        });

        let mut imports = Vec::new();
        for import in self.module.imports() {
            match import.name() {
                "memory" => imports.push(memory.into()),
                "seed" => imports.push(seed_func.into()),
                "abort" => imports.push(abort_func.into()),
                _ => return Err(anyhow!("Unknown import: {}", import.name())),
            }
        }

        let instance = Instance::new(&mut store, &self.module, &imports)?;

        // If the instance exports a memory, we should use that instead of the imported one.
        let exported_memory = instance.get_memory(&mut store, "memory");
        let active_memory = exported_memory.unwrap_or(memory);

        // Find the __new function to allocate string
        let new_func = instance
            .get_func(&mut store, "__new")
            .ok_or_else(|| anyhow!("__new function not found"))?
            .typed::<(i32, i32), i32>(&store)?;

        // Find the decrypt function
        let decrypt_func = instance
            .get_func(&mut store, "decrypt")
            .ok_or_else(|| anyhow!("decrypt function not found"))?
            .typed::<(i32, f64), i32>(&store)?;

        // Allocate string in WASM memory
        // length << 1 because UTF-16, id = 2 (string type)
        let ptr = new_func.call(&mut store, ((hex_str.len() << 1) as i32, 2))?;
        
        // Write UTF-16 characters
        let mut utf16_bytes = Vec::with_capacity(hex_str.len() * 2);
        for c in hex_str.encode_utf16() {
            utf16_bytes.extend_from_slice(&c.to_le_bytes());
        }
        active_memory.write(&mut store, ptr as usize, &utf16_bytes)?;

        // Call decrypt
        let res_ptr = decrypt_func.call(&mut store, (ptr, tmdb_id))?;

        // Read string back (res_ptr points to UTF-16 bytes, res_ptr - 4 has length in bytes)
        let mut len_bytes = [0u8; 4];
        active_memory.read(&mut store, (res_ptr - 4) as usize, &mut len_bytes)?;
        let mut len = u32::from_le_bytes(len_bytes) as usize;
        
        println!("[DEBUG] res_ptr={}, read len={}", res_ptr, len);

        if len == 0 || len > 1000000 {
            len = 2000;
        }

        let mut res_utf16_bytes = vec![0u8; len];
        active_memory.read(&mut store, res_ptr as usize, &mut res_utf16_bytes)?;

        let mut res_utf16 = Vec::with_capacity(len / 2);
        for chunk in res_utf16_bytes.chunks_exact(2) {
            res_utf16.push(u16::from_le_bytes([chunk[0], chunk[1]]));
        }

        let mut base64_str = String::from_utf16_lossy(&res_utf16);
        
        let expected_len = hex_str.len() / 2;
        if base64_str.len() > expected_len {
            let mut cut_len = expected_len;
            while cut_len > 0 && !base64_str.is_char_boundary(cut_len) {
                cut_len -= 1;
            }
            base64_str.truncate(cut_len);
        }

        Ok(base64_str)
    }
}
