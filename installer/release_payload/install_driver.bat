@echo off 
pnputil.exe /add-driver "%~dp0driver\OpenWaveLinkAudio.inf" /install 
pause 
