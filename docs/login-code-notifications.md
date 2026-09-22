# Login-code notifications

Both the fast notification assessor and full triage extract an optional
`login_code: { service, code }` from the arriving email. The notification lane
checks the auth flag, an arrival age under ten minutes, code shape and exact source occurrence, then emits
`Your <service> login code is <code>`. Leading zeros, case, spaces and hyphens
are preserved. Service names must appear case-insensitively in the sender, subject, or body, contain no Unicode control (Cc) or format (Cf) characters, and fit in 32 characters. Older arrivals retain descriptive copy. No extra model call or database migration is needed.

The model identifies the current code and service semantically. It must return
null for ambiguous codes, older quoted messages, reset links, passwords, recovery
codes and informational alerts. Code validation prevents invented or partial
codes, but does not independently prove the model's semantic classification.
Invalid or absent extraction uses the existing descriptive notification.

This deliberately makes the login code visible in notification history and on
the lock screen according to the user's notification settings. Reasons and
ordinary summaries still omit credentials. Existing email access restrictions
and notification eligibility/deduplication continue to apply.

The formatted event body reaches the existing macOS renderer and iOS
notification service extension. The relay still transports encrypted events.

## Validation

Automated tests cover exact copy through both notification lanes, leading zeros,
mixed-case and separated codes, source matching, invalid extraction fallback,
non-auth and delayed-arrival fallback through both lanes, service source matching, Unicode format/control rejection, and existing notification eligibility/deduplication.

Device acceptance check (requires a configured account and notification grants):

1. On iOS 26 and macOS 26, request a fresh login code from a real service.
2. Confirm the delivered banner body is exactly the formatted sentence, without
   the email subject or body appended.
3. Focus the service's verification field and check the OS AutoFill suggestion.
4. Repeat with the app in the foreground and background, and with iOS receiving
   the push while the app is closed.
5. Check a login alert and a magic-link email still produce descriptive text.

Apple documents verification-code AutoFill from app notifications in its
[iOS 26 feature list](https://www.apple.com/os/pdf/All_New_Features_iOS_26_Sept_2025.pdf).
Formatting makes the code available to OS detection; it cannot guarantee a
suggestion regardless of system settings, code format, or receiving app.
