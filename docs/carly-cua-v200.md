# Carly CUA-V200 adapter research and architecture notes

Status: research/reference, updated 2026-08-29.

This document consolidates OBDentic's owned hardware evidence, public Carly/FCC material,
chip-vendor documentation, the official ELM327 reference, and the current implementation
shape. The purpose is to keep the physical Carly adapter model separate from the ELM327
command dialect it happens to expose.

## Executive conclusion

The Carly CUA-V200 must not be modeled as "an ELM327 adapter".

The strongest public hardware evidence shows a Carly-designed adapter built around two
microcontrollers:

- Texas Instruments `CC2541F256` for Bluetooth Low Energy
- STMicroelectronics `STM32L431CCU6` as the main vehicle-side controller

The fully equipped `CUA-V200-CE8BD91-K78-J1` variant additionally contains six separate
CAN transceivers plus discrete ISO 9141 and SAE J1850 interfaces. There is no ELM327 IC in
the public block diagram. At the same time, owned captures show that the STM32/firmware
presents an ELM327-compatible ASCII command surface (`ATI`, `AT@1`, `ATSP0`, `ATSH`,
`ATCRA`, etc.) over a BLE serial-like GATT service.

Therefore the correct model is:

```text
Carly CUA-V200 hardware
  -> BLE transport / GATT profile
  -> Carly adapter firmware and hardware routing
  -> ELM327-compatible command dialect
  -> OBD-II / CAN / ISO-TP / UDS / KWP protocol work
  -> semantic OBDentic operations
```

not:

```text
BLE -> ELM327 hardware -> vehicle
```

For OBDentic this implies a dedicated Carly adapter backend/profile. Shared ELM command
handling should be a reusable dialect layer underneath multiple adapter implementations,
not the identity of the adapter itself.

## 1. Owned hardware evidence

The local reference unit is a Carly CUA-V200. A full Bluetooth HCI capture from the Carly
Android app has been reviewed and the raw capture remains local because it contains VIN,
adapter authentication material, and other identifiers.

Capture SHA-256:

```text
aa5df8d61df5318281f65dcd00b203a5db97f8f8b73ec900f05137c3c5a4641b
```

The reviewed session contained:

- 657 ATT packets
- 170 reconstructed request/response groups
- no observed engine-RPM request in that app session

### 1.1 BLE/GATT interface observed on the owned adapter

The adapter exposed:

- Device Information firmware revision: `Carly`
- UART-like service: `0000FFE0-0000-1000-8000-00805F9B34FB`
- UART-like characteristic: `0000FFE1-0000-1000-8000-00805F9B34FB`
- service handle range in the capture: `0x0017..0x001B`
- FFE1 declaration handle: `0x0018`
- FFE1 value handle: `0x0019`
- FFE1 CCCD: `0x001A`

Observed application traffic used:

- ATT Write Commands to handle `0x0019`
- ATT Handle Value Notifications from handle `0x0019`
- notifications enabled via the CCCD

A second vendor-specific service was present:

```text
F000FFC0-0451-4000-B000-000000000000
```

with `FFC1` and `FFC2` characteristics. It was not used for the observed vehicle traffic.
Its purpose is therefore unknown and must not be guessed or probed blindly.

### 1.2 ELM-compatible command surface observed directly

The FFE1 payload behaves as an ASCII command stream. Direct CoreBluetooth access from
macOS produced, among others:

```text
ATI
-> ELM327 v1.4 v100

AT@1
-> carly-universal v200

AT RV
-> about 12.57 V during the observed session
```

The Carly app also configured CAN headers, receive filters, flow control, and timeouts via
ordinary ELM-style `AT` commands.

This is strong evidence for ELM command compatibility, but not for an ELM327 chip. Public
hardware documentation identifies the actual controllers as CC2541 + STM32L431.

### 1.3 Direct access without the Carly app

Read-only direct access from macOS through CoreBluetooth was verified without:

- the Carly app
- a Carly subscription
- cloud access
- completing the adapter's app-level `SEED/KEY` handshake observed in the proprietary
  application flow

The authentication material from the original app capture is intentionally not published.

The owned adapter requires an explicit standard `01 00` request after `ATSP0` in the
current OBDentic path so that automatic ELM protocol detection is completed before later
semantic traffic. This matches the official ELM327 auto-search behavior described later in
this document.

A following standard `01 0C` request with ignition on and the engine stopped returned a
valid zero-RPM response.

### 1.4 Vehicle-side traffic observed through Carly

Observed engine ECU addressing included:

- request header `0x7E0`
- response header `0x7E8`

Examples from the original Carly session included:

- `10 03` DiagnosticSessionControl, positive `50 03`
- `22 F1 90` ReadDataByIdentifier for VIN
- `22 F1 9E` and `22 F1 A2` ECU identification reads
- `19 02 AE` ReadDTCInformation, including response-pending before a positive response

`10 03` is state-changing session control and remains excluded from OBDentic's read-only
allowlist even though the Carly application used it.

Other observed addressing included:

- request `0x714` / response `0x77E`
- Volkswagen TP 2.0-like traffic involving CAN ID `0x200`

The capture demonstrates that the Carly adapter is capable of more than generic SAE OBD-II
traffic. It does not prove that all such capabilities are reachable through the documented
ELM-compatible subset or safe for OBDentic to expose.

## 2. FCC identity and product variants

The public FCC filing is `2AUNDCARLY-04`, product `CARLY-04`, filed by Carly Solutions
GmbH & Co. KG in 2021.

Carly's Product Equality Declaration and the FCC RF test report list three CUA-V200
variants built on the same PCB. They use the same microcontrollers, power-supply parts,
oscillators, and RF components; the difference is the population of wired vehicle-bus
components.

### 2.1 CUA-V200-CE8BD91-K78-J1

This is the fully equipped variant:

- 6 CAN transceivers
- discrete ISO 9141 interface
- discrete SAE J1850 interface

The RF compliance tests were performed with this fully equipped J1 variant.

### 2.2 CUA-V200-CE8BD91-K78-J0

Same options as the J1 variant except:

- no SAE J1850 interface

### 2.3 CUA-V200-CE00000-K78-J0

The FCC narrative states that this variant contains:

- one CAN transceiver on OBD pins 6/14
- ISO 9141 interface components
- no other optional vehicle-bus population

The textual Product Equality Declaration and RF report are preferred over automated visual
interpretation of the block-diagram image if a rendered diagram appears ambiguous.

## 3. Public block-diagram hardware

The block diagram reproduced in the public FCC RF report identifies these major blocks in
the fully populated adapter:

```text
Power supply
   |
   +-- TI CC2541F256 SoC
   |      -> BLE / PCB antenna
   |
   +-- ST STM32L431CCU6 SoC
          |
          +-- CAN transceiver: OBD 6-14
          +-- CAN transceiver: OBD 3-8
          +-- CAN transceiver: OBD 3-11
          +-- CAN transceiver: OBD 12-13
          +-- CAN transceiver: OBD 1-9
          +-- CAN transceiver: OBD LS-1
          +-- ISO 9141 interface
          +-- SAE J1850 interface
```

The public diagram also shows 32.768 kHz, 32 MHz, and 12 MHz oscillator references. The
available public text does not justify assigning every oscillator to a specific MCU, so
OBDentic should record only their presence unless the schematic becomes publicly available.

`OBD LS-1` is the label used by the FCC block diagram. Its exact pin-level topology and
routing semantics are not publicly established by the filing and should not be expanded by
inference.

### 3.1 What is not public

The FCC record marks the following exhibits as metadata-only / long-term confidential:

- detailed block diagram exhibit
- schematics
- operational description
- parts list / tune-up information

The public RF report fortunately reproduces a useful high-level block diagram, but it does
not reveal the detailed vehicle-bus multiplexing circuitry or firmware command needed to
select the six CAN paths.

That missing information is important: the hardware clearly has multiple CAN interfaces,
but no public source found so far documents a Carly command that selects or routes them.

## 4. CC2541 BLE controller

The FCC RF report explicitly identifies the Bluetooth chipset as Texas Instruments
`CC2541`, and the public block diagram refines this to `CC2541F256`.

TI documents the CC2541F256 as a Bluetooth Low Energy / proprietary 2.4 GHz wireless MCU
with, among other capabilities:

- enhanced 8051 MCU core
- 256 KB flash on the F256 part
- 8 KB RAM
- two USARTs configurable for UART or SPI
- I2C
- AES hardware
- BLE protocol stack support
- a network-processor configuration intended for an application running on an external
  microcontroller

That external-MCU mode is consistent with the Carly two-controller topology, although the
exact CC2541-to-STM32 electrical interface is not public. UART between the two is therefore
plausible but remains an inference, not a confirmed Carly schematic fact.

### 4.1 FFE0/FFE1 is a vendor serial convention, not a standard Bluetooth service

The 16-bit FFE0/FFE1 UUID pair is widely used by CC2541/HM-10-style transparent BLE serial
firmware:

- service `FFE0`
- characteristic `FFE1`
- central writes bytes to the characteristic
- peripheral returns bytes using notifications

This closely matches the owned Carly HCI capture and makes "BLE serial-like byte channel" a
good abstraction for OBDentic.

It does **not** mean the Carly adapter is literally an HM-10 module or runs HM-10 firmware.
Carly has its own CC2541 firmware (`CarlyBLE` in the FCC report) and exposes an additional
vendor service not present in the simplest HM-10 model.

BLE notification/write packet boundaries must not be treated as ELM response boundaries.
OBDentic must accumulate the byte stream until the command-level terminator/prompt is
complete.

## 5. STM32L431 vehicle-side controller

The public block diagram identifies `STM32L431CCU6` as the second SoC.

ST documents this MCU as:

- Arm Cortex-M4
- up to 80 MHz
- 256 KB flash
- 64 KB SRAM
- one CAN 2.0A/B controller
- multiple USART/SPI/I2C peripherals

The six external CAN transceivers therefore do not imply six independent on-chip CAN
controllers. The public hardware strongly suggests external bus-path selection/multiplexing
around the STM32's vehicle interface, but the actual switch/mux implementation is hidden in
the confidential schematic.

This distinction matters for adapter design: OBDentic should model the six physical Carly
bus paths as adapter capabilities, not as six simultaneously usable CAN controllers.

## 6. FCC firmware and RF details

The 2021 FCC RF report records the tested firmware as:

```text
CC2541 firmware: CarlyBLE 1.003.0012
STM32 bootloader: 2.000.0005
STM32 main:       1.158.0150
```

The same report describes:

- equipment type: BLE device / OBD adapter
- nominal supply during RF testing: 12 V DC
- Bluetooth chipset: CC2541
- GFSK modulation
- 40 BLE channels
- 1 Mbps data rate
- frequency range 2400-2483.5 MHz, using BLE channels from 2402 through 2480 MHz
- one integrated PCB antenna
- antenna listed as Unictron `H2UB4K1H1B0100`
- antenna gain listed as -0.3 dBi
- no external antenna connector
- FCC grant maximum peak conducted output: 0.0005 W

The test report labels the Bluetooth type as "5.0 Low Energy", while TI's original CC2541
product documentation is a Bluetooth 4.0-era device/stack. OBDentic should therefore avoid
using a Bluetooth-generation label as a behavioral contract. The actual GATT behavior and
features observed on hardware are more useful than the marketing/test-report version label.

For laboratory RF modes, the filing says the device could be driven through TI SmartRF
Studio 2.21.0 using a CC debugger and a Carly Universal Adapter V2 programmer board. This is
a certification/debug path, not evidence of an application-facing diagnostic protocol.

## 7. Mechanical, electrical, and user-facing product information

The Carly user manual gives:

- input: 9-16 V DC
- maximum power: 1 W
- operating temperature: -10 C to 40 C, non-condensing
- approximate dimensions: 68.5 x 40.0 x 19.0 mm

The FCC label exhibit gives approximately 39.9 x 68.7 x 19.6 mm. The small dimensional
variation is consistent with different rounding/measurement conventions and is not
architecturally significant.

The label material identifies variants including the full
`CUA-V200-CE8BD91-K78-J1` designator.

Current Carly support material describes the black Carly Universal Scanner as working with
iOS and Android across supported brands. Carly tells users not to pair it manually in the
phone's Bluetooth settings; the app owns the connection. Their troubleshooting guidance
also describes the normal powered/advertising LED state. These are useful behavioral clues,
but OBDentic must continue to discover and connect through the actual BLE/GATT contract
rather than depend on LED behavior or app instructions.

## 8. Why `ATI -> ELM327 ...` does not identify the hardware

Elm Electronics describes the genuine ELM327 as an "OBD to RS232 Interpreter" IC that
implements an AT-command interface and supports standard OBD-II vehicle protocols including
J1850, ISO 9141-2, ISO 14230-4, and ISO 15765-4.

The public Carly block diagram, however, contains no ELM327 IC. It contains a CC2541 and an
STM32L431.

Therefore this observed response:

```text
ATI
-> ELM327 v1.4 v100
```

should be interpreted as command-dialect compatibility/emulation. The stronger Carly-specific
identity is:

```text
AT@1
-> carly-universal v200
```

High-confidence inference:

```text
Carly application ASCII command
  -> CC2541 BLE byte transport
  -> Carly STM32 firmware
  -> ELM-compatible parser / adapter control
  -> vehicle interface
```

The exact split of parsing work between CC2541 and STM32 is not public, so OBDentic should
not encode that internal division as fact.

## 9. ELM327 behavior that explains observed Carly quirks

The official ELM327 reference documents `ATSP0` as automatic protocol selection. The search
is deferred until the next OBD request.

During an automatic search, ELM327 may:

- ignore headers previously supplied by the application
- use protocol-specific default OBD headers
- send standard search requests such as `01 00`
- emit `SEARCHING...` before the eventual response

This directly explains why OBDentic previously saw Mode 01 PID 00 responses while it was
trying to perform another semantic operation immediately after `ATSP0`.

Correct architecture:

```text
adapter initialization
  -> bounded automatic-protocol establishment (`01 00` evidence)
  -> record selected/observed protocol state
  -> semantic diagnostic operation
```

The negotiation traffic is adapter/protocol preparation evidence and must not be mistaken for
the semantic job's response.

This behavior is shared ELM-dialect knowledge. It belongs in reusable ELM handling, not in
Carly hardware identity.

## 10. Carly capabilities that standard ELM semantics do not explain

The full Carly adapter has six populated CAN transceiver paths. Standard ELM commands such
as `ATSH`, `ATCRA`, flow-control commands, and `ATSP` describe message/protocol behavior but
do not by themselves document how Carly selects its non-standard physical OBD pin pairs.

No public source reviewed here identifies:

- the command used to select a Carly vehicle-bus transceiver
- whether selection is automatic, explicit, or vehicle-profile driven
- the purpose of the `F000FFC0...` BLE service
- the detailed CC2541 <-> STM32 framing
- a complete Carly-private command dictionary
- the exact implementation of the app-level authentication exchange

These are separate adapter-research questions. They must not be guessed from generic ELM327
documentation.

## 11. Architecture decision for OBDentic

### 11.1 Separate adapter identity from command dialect

Recommended dependency model:

```text
DiagnosticSession actor
        |
        v
AdapterBackend
        |
        +-- CarlyCuaV200
        |     - discovery identity
        |     - FFE0/FFE1 GATT binding
        |     - Carly identity checks
        |     - Carly-specific quirks/capabilities
        |     - future physical-bus selection if independently learned
        |
        +-- OtherAdapter...
              - its own transport/profile/quirks

Shared protocol helpers beneath/alongside the backend:

ElmDialect
  - command termination and prompt handling
  - AT command semantics
  - `ATSP0` negotiation behavior
  - header/filter/flow-control syntax where supported
  - ELM textual response normalization
```

An adapter can advertise `ElmDialect` compatibility without being represented as ELM327
hardware.

### 11.2 Suggested source split

The current `src/ble.rs` mixes several concerns:

- Bluetooth central discovery
- Carly name filtering
- Carly FFE0/FFE1 service selection
- Carly-specific error strings and identity checks
- generic ELM command exchange
- ELM initialization/protocol negotiation
- semantic session logic

A future refactor should separate these concepts, for example:

```text
src/transport/ble.rs
    generic BLE connection and byte-stream mechanics

src/adapter/mod.rs
    AdapterBackend / capability contracts

src/adapter/carly_cua_v200.rs
    Carly discovery, GATT profile, identity, adapter quirks

src/adapter/elm.rs
    reusable ELM-compatible command dialect and normalization

src/session/...
    single-owner DiagnosticSession actor and semantic operations
```

Exact module names are not important. The boundary is.

### 11.3 Current implementation evidence for the split

Today `src/ble.rs` already contains explicit Carly hardware assumptions:

- `CARLY_SERVICE = 0xFFE0`
- `CARLY_CHANNEL = 0xFFE1`
- scan results are filtered by names containing `carly`
- the connection requires the Carly FFE0 service and FFE1 characteristic
- `ATI` must look ELM-compatible
- `AT@1` must identify `CARLY-UNIVERSAL`

The same file then implements generic `ElmExchange` behavior and ELM auto-protocol logic.
That works for the first hardware path, but it is precisely the coupling that should be
removed before adding a second ELM-compatible adapter family.

## 12. Safety boundary

A dedicated Carly backend must not become a private/raw Carly command console.

The existing invariant remains:

```text
semantic operation
  -> closed Vehicle/Protocol Knowledge
  -> read-only safety policy
  -> AdapterBackend
  -> exact transport operation
```

Never:

```text
CLI / TUI / MCP
  -> arbitrary ELM text
  -> arbitrary Carly vendor command
  -> vehicle
```

Carly hardware may support coding, adaptation, session changes, clearing, active tests, or
other write-capable operations. OBDentic must not expose those merely because the adapter can
perform them.

Likewise, discovery of a Carly-private bus-routing command would be adapter knowledge only.
It would not authorize arbitrary traffic on that bus.

## 13. Evidence taxonomy

To keep future research auditable, classify adapter facts separately:

### VERIFIED / OWNED HARDWARE

Examples:

- FFE0/FFE1 used for the observed command stream
- FFE1 write/notify behavior and captured handles
- `ATI`, `AT@1`, `AT RV` responses
- direct macOS read-only access
- observed 7E0/7E8, 714/77E, and TP2.0-like traffic

### VERIFIED / PUBLIC MANUFACTURER OR CERTIFICATION

Examples:

- CC2541F256 + STM32L431CCU6 block diagram
- CUA-V200 variant definitions
- six CAN transceivers on the J1 variant
- ISO 9141 / J1850 population
- FCC firmware versions and RF characteristics

### CORROBORATED / EXTERNAL PLATFORM KNOWLEDGE

Examples:

- FFE0/FFE1 as a common CC2541/HM-10-style transparent serial pattern
- CC2541 UART/network-processor capability
- STM32L431 CAN and UART capabilities
- ELM327 `ATSP0` auto-search behavior

### INFERRED

Examples:

- the STM32 implements most/all of the ELM-compatible parser
- CC2541 and STM32 communicate internally over UART
- the six Carly CAN transceivers are selected by a mux controlled by the STM32

These inferences are technically plausible but should stay labeled until direct evidence is
available.

## 14. Recommended follow-up research

Safe adapter-only research can continue without sending unknown diagnostic requests to a
vehicle:

1. Enumerate and persist the complete GATT table from the owned CUA-V200.
2. Record advertisement data, local name, service UUIDs, characteristic properties, MTU,
   write types, and notification behavior.
3. Build a bounded ELM compatibility matrix using known non-vehicle-changing `AT` commands.
4. Compare Carly responses with the official ELM327 command/version matrix; record quirks
   rather than assuming version-number compatibility.
5. Observe adapter behavior around `ATZ`, `ATD`, `ATSP0`, `ATDP`, `ATDPN`, headers, receive
   filters, flow-control configuration, and prompt framing.
6. Continue passive analysis of owned Carly app captures for evidence of physical-bus
   selection. Do not replay unknown vendor commands until their effect is understood and
   proven read-only.
7. Treat the second `F000FFC0...` service as unknown until passive evidence identifies it.
8. Once the adapter seam exists, keep Carly-specific hardware/capability tests separate from
   reusable ELM-dialect conformance tests.

## 15. Source and document inventory

The following references were reviewed for this consolidation. Some FCC exhibits are public
only as photos/metadata; the detailed schematic/operational exhibits are long-term
confidential and therefore cannot be treated as available evidence.

### OBDentic owned evidence and code

- Existing owned-capture notes (this file's previous revision):
  https://github.com/frankherchet/obdentic/blob/main/docs/carly-cua-v200.md
- Current BLE/Carly/ELM implementation:
  https://github.com/frankherchet/obdentic/blob/main/src/ble.rs
- OBDium clean-room communication research:
  https://github.com/frankherchet/obdentic/blob/main/docs/research/obdium-communication-reference.md

### Carly / FCC filing

- FCC application index, `2AUNDCARLY-04`:
  https://fccid.io/2AUNDCARLY04
- FCC mirror/index:
  https://fcc.report/FCC-ID/2AUNDCARLY-04
- Product Equality Declaration, document 5454412:
  https://fcc.report/FCC-ID/2AUNDCARLY-04/5454412.pdf
- RF Test Report `80080584-02 Rev_1`, document 5454408:
  https://fcc.report/FCC-ID/2AUNDCARLY-04/5454408.pdf
- User Manual, document 5454405:
  https://fcc.report/FCC-ID/2AUNDCARLY-04/5454405.pdf
- Internal Photos, document 5454403:
  https://fccid.io/2AUNDCARLY04/Internal-Photos/Internal-Photos-5454403
- External Photos, document 5454402:
  https://fccid.io/2AUNDCARLY-04/External-Photos/External-Photos-5454402
- ID Label / location, document 5454404:
  https://fccid.io/2AUNDCARLY-04/Label/ID-Lable-Location-Info-5454404
- Test Setup Photos, document 5454410:
  https://fccid.io/2AUNDCARLY04/Test-Setup-Photos/Test-Setup-Photos-5454410
- Test Report Annex, document 5454409:
  https://fccid.io/2AUNDCARLY04/Test-Report/Test-Report-Annex-5454409
- Long-term-confidentiality request, document 5454401:
  https://fccid.io/2AUNDCARLY-04/Letter/LTC-Request-Letter-5454401
- Agency Letter, document 5454411:
  https://fccid.io/2AUNDCARLY-04/Letter/Agency-Letter-5454411
- Product Equality Declaration metadata page:
  https://fccid.io/2AUNDCARLY-04/Letter/Product-Equality-Declaration-5454412

The FCC application index also lists metadata-only long-term-confidential exhibits for the
full block diagram, schematics, operational description, and parts list. Those files are not
publicly inspectable and must not be treated as if their contents were known.

### Carly current public product/support material

- Carly technical-information / conformity page:
  https://www.mycarly.com/technical-information/
- Current black Universal Adapter connection guidance:
  https://support.mycarly.com/hc/de/articles/19920785568530-Schwarzer-Carly-Universal-Adapter-Aktuelles-Modell-Wie-kann-ich-Verbindungsprobleme-l%C3%B6sen
- Carly base-package description / one adapter across supported brands:
  https://support.mycarly.com/hc/de/articles/20084584381074-Was-ist-das-Carly-Basis-Paket

### Chip-vendor documentation

- TI CC2541 product page:
  https://www.ti.com/product/CC2541
- TI CC2541 datasheet:
  https://www.ti.com/lit/ds/symlink/cc2541.pdf
- ST STM32L431CC product page:
  https://www.st.com/en/microcontrollers-microprocessors/stm32l431cc.html

### ELM327 reference

- Elm Electronics OBD/ELM product overview:
  https://www.elmelectronics.com/products/ics/obd/
- Official ELM327 data sheet / command reference:
  https://elmelectronics.com/wp-content/uploads/2020/05/ELM327DSL.pdf

### FFE0/FFE1 corroboration

These sources are used only to corroborate that FFE0/FFE1 is a common CC2541/HM-10-style
transparent serial convention. They are not evidence that Carly runs HM-10 firmware.

- Keyestudio HM-10 / CC2541 documentation:
  https://docs.keyestudio.com/projects/KS0174/en/latest/docs/KS0174.html
- Farnell-hosted HM-10 shield manual referencing CC2541 and FFE0/FFE1:
  https://www.farnell.com/datasheets/4406236.pdf

## Decision

OBDentic should introduce a first-class `CarlyCuaV200` adapter backend/profile before
support for additional adapter families grows further.

`ELM327` should describe a reusable command dialect/protocol compatibility layer, not the
physical adapter identity. This preserves the facts that:

- Carly is a distinct multi-bus hardware platform
- its BLE/GATT contract is Carly-specific
- its firmware only emulates/implements an ELM-compatible surface
- future ELM-compatible devices may have different BLE services, firmware quirks, protocol
  timing, physical interfaces, and capabilities
- adapter-specific features must never leak into generic Protocol Knowledge or bypass the
  read-only semantic safety boundary
