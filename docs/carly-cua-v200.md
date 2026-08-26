# Carly CUA-V200 capture notes

Confirmed from a full Bluetooth HCI capture on a Mi 9T running Android 10.

## BLE interface

- Device address during the capture: redacted
- Device Information firmware revision (handle `0x0014`): `Carly`
- UART service: `0xFFE0`, handles `0x0017..0x001B`
- UART characteristic: `0xFFE1`, declaration handle `0x0018`, value handle `0x0019`
- Notifications are enabled through CCCD handle `0x001A`
- Requests are ATT Write Commands to handle `0x0019`
- Responses are ATT Handle Value Notifications from handle `0x0019`
- A second vendor service exists at `F000FFC0-0451-4000-B000-000000000000`, with `FFC1` and `FFC2` characteristics. Carly did not use it for the observed vehicle traffic.

## Adapter protocol

The `FFE1` payload is an ASCII ELM327-style command stream:

- `ATI` returned `ELM327 v1.4 v100`.
- `AT@1` returned `carly-universal v200`.
- `AT RV` returned approximately `12.57 V` during the session.
- Carly configured CAN headers, receive filters, flow control and timeouts with ordinary `AT` commands.
- Vehicle requests used both raw CAN/KWP-style frames and UDS over ISO-TP.

## Direct access without Carly

Direct read-only access from macOS through CoreBluetooth was verified without the Carly app, a subscription, cloud access or the adapter's `SEED/KEY` handshake.

The adapter clone requires one explicit `0100` request after `ATSP0` to complete automatic protocol detection. A following standard `010C` request returned `41 0C 00 00`, correctly decoded as `0 rpm` with ignition on and the engine stopped.

Observed UDS examples against engine ECU request ID `0x7E0`, response ID `0x7E8`:

- `10 03` — DiagnosticSessionControl, positive response `50 03`; changes the ECU diagnostic session and is excluded from OBDentic's read-only allowlist.
- `22 F1 90` — ReadDataByIdentifier for VIN; value deliberately omitted here.
- `22 F1 9E` and `22 F1 A2` — ECU identification data.
- `19 02 AE` — ReadDTCInformation; the ECU first returned response-pending, then a positive response.

Other observed ECU addressing included request ID `0x714` / response ID `0x77E` and Volkswagen TP 2.0-style traffic through CAN ID `0x200`.

## Capture handling

The full capture contains adapter authentication material, identifiers and the vehicle VIN. It is stored only under the git-ignored `captures/` directory and must not be committed or shared unredacted.

Capture SHA-256: `aa5df8d61df5318281f65dcd00b203a5db97f8f8b73ec900f05137c3c5a4641b`

The session contained 657 ATT packets and 170 reconstructed request/response groups. No engine-RPM request was observed.
