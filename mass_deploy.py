import os
import json
import subprocess
import binascii
import base64
import hashlib
from cryptography.hazmat.primitives.asymmetric import ed25519

# Constants
PRIVATE_KEY_HEX = "b1a1c85c8b3f7bc471a6f88748ddf568fd6fedff1e739a9a1d31cd0d5f7ae512"
REPO_DIR = r"d:\DeluluStream\addons-repo"
# We only bump legacy addons (not scrapy since it's already v1.0.0 and deployed)
ADDONS = ['embegator', 'flexy', 'meta-resolver', 'motherbox', 'spectre', 'vandal']

def sign_manifest_data(manifest_obj):
    # Fields used for canonical payload in Rust
    fields = [
        "id", "name", "version", "protocolVersion", "publisher", 
        "publicKeyId", "platformAssets", "capabilities", 
        "headerDefaults", "minAppVersion", "releaseNotesUrl", "homepageUrl"
    ]
    
    # Create canonical object (alphabetical keys as per BTreeMap in Rust)
    canonical_obj = {}
    for field in fields:
        canonical_obj[field] = manifest_obj.get(field)

    # Encode as JSON (alphabetical keys, no whitespace)
    payload = json.dumps(canonical_obj, sort_keys=True, separators=(',', ':')).encode('utf-8')

    # Load private key
    priv_bytes = binascii.unhexlify(PRIVATE_KEY_HEX)
    private_key = ed25519.Ed25519PrivateKey.from_private_bytes(priv_bytes)
    
    # Sign
    signature = private_key.sign(payload)
    return base64.b64encode(signature).decode('utf-8')

def bump_version(current_version):
    parts = current_version.split('.')
    patch = int(parts[2])
    parts[2] = str(patch + 1)
    return ".".join(parts)

def get_hash(exe_path):
    sha256_hash = hashlib.sha256()
    with open(exe_path, "rb") as f:
        for byte_block in iter(lambda: f.read(4096), b""):
            sha256_hash.update(byte_block)
    return sha256_hash.hexdigest().lower()

def run_cmd(cmd, cwd):
    print(f"[*] Running: {cmd}")
    result = subprocess.run(cmd, cwd=cwd, shell=True)
    if result.returncode != 0:
        print(f"[!] Command failed: {cmd}")
        exit(1)

def main():
    for addon in ADDONS:
        print(f"\n======================================")
        print(f"Deploying {addon}")
        print(f"======================================")
        
        addon_dir = os.path.join(REPO_DIR, addon)
        manifest_path = os.path.join(addon_dir, 'manifest.json')
        
        with open(manifest_path, 'r', encoding='utf-8') as f:
            manifest = json.load(f)
            
        old_version = manifest['version']
        new_version = bump_version(old_version)
        print(f"[*] Bumping version: {old_version} -> {new_version}")
        
        # 1. Update Manifest
        manifest['version'] = new_version
        
        # Add http_stream_cinema capability if not present
        if 'capabilities' in manifest:
            if 'http_stream_cinema' not in manifest['capabilities']:
                manifest['capabilities']['http_stream_cinema'] = False
                
        # Update URLs with new version string
        manifest['releaseNotesUrl'] = manifest['releaseNotesUrl'].replace(old_version, new_version)
        for platform, asset in manifest['platformAssets'].items():
            asset['downloadUrl'] = asset['downloadUrl'].replace(old_version, new_version)
            
        # 2. Build Rust Binary
        print(f"[*] Building release binary for {addon}...")
        run_cmd("cargo build --release", cwd=addon_dir)
        
        # 3. Calculate Hash
        built_exe = os.path.join(addon_dir, "target", "release", f"{addon}.exe")
        if not os.path.exists(built_exe):
            print(f"[!] Built exe not found at {built_exe}")
            exit(1)
            
        print(f"[*] Calculating SHA256 hash...")
        sha256 = get_hash(built_exe)
        print(f"    Hash: {sha256}")
        
        for platform, asset in manifest['platformAssets'].items():
            asset['sha256'] = sha256
            
        # 4. Sign Manifest
        print(f"[*] Signing manifest...")
        manifest['publicKeyId'] = "delulu-official-v1"
        manifest['signature'] = sign_manifest_data(manifest)
        
        with open(manifest_path, 'w', encoding='utf-8') as f:
            json.dump(manifest, f, indent=2)
            
        # 5. Create GitHub Release
        # Determine tag format
        if addon in ['embegator', 'spectre']:
            tag = f"v{new_version}"
        else:
            tag = f"{addon}-v{new_version}"
            
        print(f"[*] Creating GitHub Release ({tag})...")
        binary_name = list(manifest['platformAssets'].values())[0]['binaryName']
        gh_cmd = f'gh release create {tag} "{built_exe}#{binary_name}" -R ZacKXSnydeR/DELULU----ADDONS -t "{addon.capitalize()} v{new_version}" --generate-notes'
        run_cmd(gh_cmd, cwd=addon_dir)
        
        print(f"[+] {addon} successfully deployed!")
        
    print("\n======================================")
    print("ALL ADDONS SUCCESSFULLY DEPLOYED")
    print("======================================")
    
if __name__ == "__main__":
    main()
