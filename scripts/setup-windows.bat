@echo off
rem 日本語のメッセージを文字化けさせないよう、コードページを UTF-8 にする。
chcp 65001 >nul
rem vw の開発環境を揃える。詳しくは setup-windows.ps1 を参照。
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0setup-windows.ps1" %*
if errorlevel 1 (
  echo.
  echo 失敗しました。上のメッセージを確認してください。
)
pause
