@echo off
echo Stopping old ZevDetector service...
net stop ZevDetector
timeout /t 1 /nobreak >nul

echo Updating binaries to v1.5.0...
copy /y "C:\Users\user\Downloads\zev-detector\zev-detector-bundle\agent\target\release\zev-detector.exe" "C:\Users\user\Downloads\zev-detector\zev-detector-bundle\agent\dist\Zev-Detector-v1.5.0-standalone\zev-detector.exe"
copy /y "C:\Users\user\Downloads\zev-detector\zev-detector-bundle\agent\target\release\zev-detector-ui.exe" "C:\Users\user\Downloads\zev-detector\zev-detector-bundle\agent\dist\Zev-Detector-v1.5.0-standalone\zev-detector-ui.exe"
copy /y "C:\Users\user\Downloads\zev-detector\zev-detector-bundle\agent\show_alert.ps1" "C:\Users\user\Downloads\zev-detector\zev-detector-bundle\agent\dist\Zev-Detector-v1.5.0-standalone\show_alert.ps1"
copy /y "C:\Users\user\Downloads\zev-detector\zev-detector-bundle\agent\show_alert.ps1" "C:\ProgramData\Zev Detector\show_alert.ps1"

echo Reconfiguring service path to v1.5.0...
sc.exe config ZevDetector binPath= "C:\Users\user\Downloads\zev-detector\zev-detector-bundle\agent\dist\Zev-Detector-v1.5.0-standalone\zev-detector.exe"

echo Starting ZevDetector v1.5.0 service...
net start ZevDetector
echo Done! Service updated to v1.5.0 successfully.
pause
