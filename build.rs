//! Writes the Windows resources (icon, version information and manifest) into a .res file the linker takes
//! directly, so building needs no resource compiler.

use std::path::Path;

const ICON: &str = "assets/Snowdrift.ico";
const RT_ICON: u16 = 3;
const RT_GROUP_ICON: u16 = 14;
const RT_VERSION: u16 = 16;
const RT_MANIFEST: u16 = 24;
const ENGLISH: u16 = 0x0409;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={ICON}");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let version = env!("CARGO_PKG_VERSION");
    let icon_file = std::fs::read(ICON).expect("icon");
    for (binary, description) in [("Snowdrift", "Snowdrift"), ("Snowdrift-Setup", "Snowdrift Setup")] {
        // A .res file opens with an empty entry.
        let mut res = vec![0, 0, 0, 0, 32, 0, 0, 0, 0xFF, 0xFF, 0, 0, 0xFF, 0xFF, 0, 0];
        res.extend([0; 16]);
        icon(&mut res, &icon_file);
        entry(&mut res, RT_VERSION, 1, ENGLISH, &version_info(version, description, &format!("{binary}.exe")));
        entry(&mut res, RT_MANIFEST, 1, ENGLISH, manifest(version).as_bytes());

        let path = Path::new(&std::env::var("OUT_DIR").unwrap()).join(format!("{binary}.res"));
        std::fs::write(&path, res).expect("resources");
        println!("cargo:rustc-link-arg-bin={binary}={}", path.display());
    }
}

/// One resource: a header naming its type and id, then the data, padded to a 32-bit boundary.
fn entry(res: &mut Vec<u8>, kind: u16, name: u16, language: u16, data: &[u8]) {
    res.extend((data.len() as u32).to_le_bytes());
    res.extend(32u32.to_le_bytes());
    for word in [0xFFFF, kind, 0xFFFF, name] {
        res.extend(word.to_le_bytes());
    }
    res.extend(0u32.to_le_bytes());
    res.extend(0x1030u16.to_le_bytes());
    res.extend(language.to_le_bytes());
    res.extend([0u8; 8]);
    res.extend(data);
    pad(res);
}

/// Each image of the .ico becomes an icon resource; a group lists them with resource ids in place of file offsets.
fn icon(res: &mut Vec<u8>, file: &[u8]) {
    let count = u16::from_le_bytes([file[4], file[5]]) as usize;
    let mut group = file[..6].to_vec();
    for i in 0..count {
        let record = &file[6 + 16 * i..6 + 16 * (i + 1)];
        let length = u32::from_le_bytes(record[8..12].try_into().unwrap()) as usize;
        let offset = u32::from_le_bytes(record[12..16].try_into().unwrap()) as usize;
        let id = i as u16 + 1;
        entry(res, RT_ICON, id, ENGLISH, &file[offset..offset + length]);
        group.extend(&record[..12]);
        group.extend(id.to_le_bytes());
    }
    entry(res, RT_GROUP_ICON, 1, ENGLISH, &group);
}

fn version_info(version: &str, description: &str, filename: &str) -> Vec<u8> {
    let parts: Vec<u32> = version.split('.').map(|part| part.parse().unwrap_or(0)).chain([0; 4]).take(4).collect();
    let (high, low) = (parts[0] << 16 | parts[1], parts[2] << 16 | parts[3]);
    let mut fixed = Vec::new();
    for value in [0xFEEF04BD, 0x0001_0000, high, low, high, low, 0x3F, 0, 0x0004_0004, 1, 0, 0, 0] {
        fixed.extend((value as u32).to_le_bytes());
    }
    let strings = [
        ("CompanyName", "Snowdrift"),
        ("FileDescription", description),
        ("FileVersion", version),
        ("InternalName", "Snowdrift"),
        ("LegalCopyright", "© 2026 Snowdrift"),
        ("OriginalFilename", filename),
        ("ProductName", "Snowdrift"),
        ("ProductVersion", version),
    ]
    .map(|(key, text)| {
        let value = wide(text);
        block(key, &value, (value.len() / 2) as u16, true, &[])
    });
    let table = block("040904B0", &[], 0, true, &strings);
    let string_info = block("StringFileInfo", &[], 0, true, &[table]);
    let translation = block("Translation", &[0x09, 0x04, 0xB0, 0x04], 4, false, &[]);
    let var_info = block("VarFileInfo", &[], 0, true, &[translation]);
    block("VS_VERSION_INFO", &fixed, fixed.len() as u16, false, &[string_info, var_info])
}

/// A version-information block: its length, value length and type, its key, its value and its children,
/// each starting on a 32-bit boundary.
fn block(key: &str, value: &[u8], value_length: u16, text: bool, children: &[Vec<u8>]) -> Vec<u8> {
    let mut block = vec![0, 0];
    block.extend(value_length.to_le_bytes());
    block.extend((text as u16).to_le_bytes());
    block.extend(wide(key));
    pad(&mut block);
    block.extend(value);
    for child in children {
        pad(&mut block);
        block.extend(child);
    }
    let length = (block.len() as u16).to_le_bytes();
    block[..2].copy_from_slice(&length);
    block
}

fn manifest(version: &str) -> String {
    let parts: Vec<&str> = version.split('.').chain(["0"; 4]).take(4).collect();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <assemblyIdentity type="win32" name="Snowdrift" version="{}"/>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <supportedOS Id="{{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}}"/>
    </application>
  </compatibility>
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true/pm</dpiAware>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2</dpiAwareness>
    </windowsSettings>
  </application>
  <dependency>
    <dependentAssembly>
      <assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0" processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*"/>
    </dependentAssembly>
  </dependency>
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="asInvoker" uiAccess="false"/>
      </requestedPrivileges>
    </security>
  </trustInfo>
</assembly>
"#,
        parts.join(".")
    )
}

fn wide(text: &str) -> Vec<u8> {
    text.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect()
}

fn pad(bytes: &mut Vec<u8>) {
    while bytes.len() % 4 != 0 {
        bytes.push(0);
    }
}
