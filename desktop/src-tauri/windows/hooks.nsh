; kitedrop installer hooks (design §8: Private-profile firewall rule, classic "Send to" entry).

!macro NSIS_HOOK_POSTINSTALL
  ; Inbound rule for the app, Private profile only. Remove an old one first so reinstalling is clean.
  nsExec::ExecToLog 'netsh advfirewall firewall delete rule name="kitedrop"'
  nsExec::ExecToLog 'netsh advfirewall firewall add rule name="kitedrop" dir=in action=allow program="$INSTDIR\kitedrop.exe" enable=yes profile=private'
  ; "Send to" menu entry for the installing user.
  CreateShortCut "$SENDTO\kitedrop.lnk" "$INSTDIR\kitedrop.exe" "--send"
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  nsExec::ExecToLog 'netsh advfirewall firewall delete rule name="kitedrop"'
  Delete "$SENDTO\kitedrop.lnk"
!macroend
