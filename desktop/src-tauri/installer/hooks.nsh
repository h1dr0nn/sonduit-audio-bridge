; Firewall rules for Sonduit.
;
; Sonduit receives nothing over Wi-Fi, but it does listen for discovery replies
; on UDP 4011, and a reply is inbound traffic. Without a rule Windows drops it
; silently: the scan finds nothing and there is no error anywhere to explain
; why.
;
; The USB case is worse. A tethered adapter lands on the Public profile, which
; blocks all inbound by default, so a first run over USB looks like a total
; failure. The rules therefore cover all three profiles.
;
; Nothing here opens the audio port. Audio only ever leaves this machine.

!macro NSIS_HOOK_POSTINSTALL
  DetailPrint "Adding firewall rules for discovery replies"

  ; Remove first, so reinstalling does not accumulate duplicate rules.
  nsExec::ExecToLog 'netsh advfirewall firewall delete rule name="Sonduit discovery"'
  Pop $0

  nsExec::ExecToLog 'netsh advfirewall firewall add rule \
    name="Sonduit discovery" \
    dir=in action=allow protocol=UDP localport=4011 \
    profile=domain,private,public \
    description="Lets Sonduit receive replies when it looks for a paired phone."'
  Pop $0
  ${If} $0 != 0
    ; Not fatal. The user can add the rule by hand, or Windows will prompt on
    ; first run; failing the whole install over it would be worse.
    DetailPrint "Could not add the firewall rule (code $0). Discovery may not find a phone until it is added."
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  DetailPrint "Removing firewall rules"
  nsExec::ExecToLog 'netsh advfirewall firewall delete rule name="Sonduit discovery"'
  Pop $0
!macroend
