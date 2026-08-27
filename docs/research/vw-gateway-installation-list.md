# VW gateway installation list: evidence review

Status: research only (2026-08-27). No diagnostic request is implemented or
sent as a result of this document.

## Question

Can OBDentic issue one known, read-only request to a Volkswagen gateway and
decode a portable mapping of installed control units for the EA189 target
profile?

## Evidence scale

| Level | Meaning | Use in OBDentic |
| --- | --- | --- |
| E3 | Normative or manufacturer documentation | Establishes protocol/function semantics, not a vehicle-specific byte map by itself. |
| E2 | Public documentation from a diagnostic-tool vendor or a reproducible, vehicle-specific capture | Supports a profiled implementation when vehicle family, gateway software and transport are identified. |
| E1 | Public reverse engineering without independent target-vehicle validation | Research lead only; never a generic probe. |
| E0 | Forum, video, or anecdotal claim without a reproducible request/response | Not implementation evidence. |

## Findings

### 1. The installation list is a gateway/bus-master function (E2/E3)

Ross-Tech documents an `Installation List` function under address 19 (CAN
Gateway). It says the function is available only on gateways that support an
installation list, and that the list can be viewed separately from coding. The
same page explicitly describes writing the list as a separate coding action:
[Ross-Tech Gateway Installation List](https://www.ross-tech.com/vcds/tour/installation-list.php).

The public mirror of the Volkswagen ODIS Engineering manual describes a
diagnostic start-up that evaluates one or more bus-master installation lists,
including multiple responding bus masters and both KWP2000 and UDS systems.
It also documents the gateway-information fields `GatewayComponentList
BusIdentifier (0x2A2D)` and `GatewayComponentList DiagProt (0x2A29)`, while the
control-unit column contains a diagnostic or node address:
[ODIS Engineering manual, chapters 4.9 and 23](https://mofler.com/uploads/attachments/monthly_2015_04/Manual_engineering-en_GB.pdf.feaf8d1e793a833a9263ed0c72724b75).

This establishes that the UI concept is real and that the result contains more
than a simple installed/not-installed bit. It does **not** publish one
vehicle-independent UDS request, one response layout, or one slot-to-address
mapping.

### 2. UDS `0x22` is a read service, but the DID is profile-specific (E3)

The AUTOSAR Diagnostic Communication Manager specification identifies UDS
service `0x22` as `ReadDataByIdentifier` and treats the supported data
identifier as ECU configuration. In other words, the service semantics are
standard, but the set and layout of supported DIDs are supplied by the ECU
project:
[AUTOSAR Diagnostic Communication Manager specification](https://www.autosar.org/fileadmin/standards/R22-11/CP/AUTOSAR_SWS_DiagnosticCommunicationManager.pdf).

Therefore “send `0x22`” is not a complete, known request. A DID, target
address, diagnostic session and transport path still have to be established
for the exact gateway profile. No generic DID scan is acceptable for the
read-only allowlist.

### 3. One exact mapping exists in public reverse engineering, but only for a different platform (E1)

The public VAG-CP-Docs research for a **2013 Audi A6 C7 (4G0), gateway
4G0907468AC, software 0037** reports a gateway constellation DID `0x04A3` and
the related DIDs `0x2A26`, `0x2A2A` and `0x2A2C`. It describes `0x2A2A` as an
80-byte allocation table (one VCDS module address per slot) and `0x2A2C` as a
160-byte transport-identifier table:

- [J533 constellation service and DID research](https://github.com/dspl1236/VAG-CP-Docs/blob/main/technical/j533-constellation-deep-dive.md)
- [J533 slot map and live-vehicle cross-reference](https://github.com/dspl1236/VAG-CP-Docs/blob/main/technical/j533-slot-map-decoded.md)

This is useful evidence that a profiled implementation can exist. It is not
evidence for a VW EA189 gateway: the vehicle, gateway part number, software,
bus topology, slot contents and transport availability differ. The same
research explicitly reports that many slots are routed through VW TP 2.0,
KWP2000 or LIN rather than direct ISO-TP CAN.

The corresponding read service would be represented at the UDS application
layer as `22 04 A3` **for that documented Audi C7 profile only**. The public
material does not establish that request, its addressing/session setup, or its
layout for EA189.

### 4. Target-profile evidence is absent

The repository's existing Carly notes document observed UDS traffic and
several ECU address pairs, but no gateway installation-list request or
decoded gateway slot map. They are therefore not sufficient to promote an
EA189 gateway request into the read-only vocabulary.

## Decision

**No: an exact read-only gateway request and mapping are not sufficiently
evidenced for the VW/EA189 profile.**

The evidence supports only this bounded statement: some VW-group gateways
expose installation-list data through profile-specific diagnostic functions;
the public Audi C7 research gives one concrete example, but it cannot be
ported to EA189 by guessing a DID or copying its slot map.

## Binding safety conclusion

Until target-profile evidence exists, OBDentic must not send a guessed gateway
request, a generic `0x22` DID scan, or any of the Audi-C7 DIDs (`0x04A3`,
`0x2A26`, `0x2A2A`, `0x2A2C`) to a VW vehicle. It must not send installation-list
coding (`0x2E`) or any session/routing sequence intended to reach an unknown
gateway. This research produces no live request.

An implementation may be reconsidered only after a redacted, reproducible
capture or manufacturer profile identifies all of the following for the target
gateway: part number/software family, request and response addressing,
required diagnostic session and transport, exact read DID(s), response
segmentation/layout, and an independently checked slot-to-diagnostic-address
mapping. Until then, gateway topology remains out of scope and the milestone
must stay open.
