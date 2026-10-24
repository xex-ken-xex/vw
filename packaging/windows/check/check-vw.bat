@echo off
rem 日本語のメッセージを文字化けさせないよう、コードページを UTF-8 にする。
chcp 65001 >nul
rem スクリプトの実行が禁止された PC でも、この起動に限って許す。
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0check-vw.ps1" %*
pause
