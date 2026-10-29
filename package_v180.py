import os
import shutil
import subprocess
import zipfile

agent_dir = r"C:\Users\user\Downloads\zev-detector\zev-detector-bundle\agent"
target_rel = os.path.join(agent_dir, r"target\release")
dist_dir = os.path.join(agent_dir, "dist")
standalone_dir = os.path.join(dist_dir, "Zev-Detector-v1.8.0-standalone")
os.makedirs(standalone_dir, exist_ok=True)

# Copy newly built binaries
if os.path.exists(os.path.join(target_rel, "zev-detector.exe")):
    shutil.copy2(os.path.join(target_rel, "zev-detector.exe"), os.path.join(standalone_dir, "zev-detector.exe"))
if os.path.exists(os.path.join(target_rel, "zev-detector-ui.exe")):
    shutil.copy2(os.path.join(target_rel, "zev-detector-ui.exe"), os.path.join(standalone_dir, "zev-detector-ui.exe"))
if os.path.exists(os.path.join(agent_dir, "show_alert.ps1")):
    shutil.copy2(os.path.join(agent_dir, "show_alert.ps1"), os.path.join(standalone_dir, "show_alert.ps1"))
if os.path.exists(os.path.join(agent_dir, "config.sample.toml")):
    shutil.copy2(os.path.join(agent_dir, "config.sample.toml"), os.path.join(standalone_dir, "config.sample.toml"))

# Update C:\ProgramData\Zev Detector\show_alert.ps1
pdata = r"C:\ProgramData\Zev Detector"
os.makedirs(pdata, exist_ok=True)
if os.path.exists(os.path.join(agent_dir, "show_alert.ps1")):
    shutil.copy2(os.path.join(agent_dir, "show_alert.ps1"), os.path.join(pdata, "show_alert.ps1"))

# Re-create ZIP
zip_path = os.path.join(dist_dir, "Zev-Detector-v1.8.0-standalone.zip")
with zipfile.ZipFile(zip_path, 'w', zipfile.ZIP_DEFLATED) as zf:
    for root, dirs, files in os.walk(standalone_dir):
        for f in files:
            full = os.path.join(root, f)
            rel = os.path.relpath(full, standalone_dir)
            zf.write(full, rel)
print(f"Updated ZIP: {zip_path} ({os.path.getsize(zip_path)} bytes)")

# Compile Inno Setup installer if available
iscc = r"C:\Program Files (x86)\Inno Setup 6\ISCC.exe"
iss_file = os.path.join(agent_dir, "installer", "setup.iss")
if os.path.exists(iscc) and os.path.exists(iss_file):
    res = subprocess.run([iscc, iss_file], capture_output=True, text=True)
    if res.returncode == 0:
        print("Inno Setup compiled successfully!")
    else:
        print("Inno Setup error:", res.stderr)
else:
    print("ISCC not found, standalone zip ready.")
