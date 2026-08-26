#!/usr/bin/env swift

import CoreBluetooth
import Darwin
import Foundation

final class CarlyProbe: NSObject, CBCentralManagerDelegate, CBPeripheralDelegate {
    private let serviceID = CBUUID(string: "FFE0")
    private let characteristicID = CBUUID(string: "FFE1")
    private let commands = ["ATI\r", "ATZ\r", "ATE0\r", "ATL0\r", "ATS0\r", "ATH0\r", "ATSP0\r", "0100\r", "010C\r"]
    private var central: CBCentralManager!
    private var adapter: CBPeripheral?
    private var channel: CBCharacteristic?
    private var commandIndex = 0
    private var response = Data()

    override init() {
        super.init()
        central = CBCentralManager(delegate: self, queue: nil)
        DispatchQueue.main.asyncAfter(deadline: .now() + 30) { self.finish("timeout", status: 1) }
    }

    func centralManagerDidUpdateState(_ central: CBCentralManager) {
        guard central.state == .poweredOn else {
            if central.state != .unknown && central.state != .resetting {
                finish("Bluetooth unavailable: \(central.state.rawValue)", status: 1)
            }
            return
        }
        print("scan      Carly BLE adapter")
        central.scanForPeripherals(withServices: nil)
    }

    func centralManager(
        _ central: CBCentralManager,
        didDiscover peripheral: CBPeripheral,
        advertisementData: [String: Any],
        rssi RSSI: NSNumber
    ) {
        let name = peripheral.name ?? advertisementData[CBAdvertisementDataLocalNameKey] as? String ?? ""
        guard name.localizedCaseInsensitiveContains("carly") else { return }
        print("adapter   \(name) (\(peripheral.identifier), RSSI \(RSSI))")
        central.stopScan()
        adapter = peripheral
        peripheral.delegate = self
        central.connect(peripheral)
    }

    func centralManager(_ central: CBCentralManager, didConnect peripheral: CBPeripheral) {
        print("connected")
        peripheral.discoverServices([serviceID])
    }

    func centralManager(_ central: CBCentralManager, didFailToConnect peripheral: CBPeripheral, error: Error?) {
        finish("connection failed: \(error?.localizedDescription ?? "unknown error")", status: 1)
    }

    func peripheral(_ peripheral: CBPeripheral, didDiscoverServices error: Error?) {
        guard error == nil, let service = peripheral.services?.first(where: { $0.uuid == serviceID }) else {
            finish("FFE0 service unavailable: \(error?.localizedDescription ?? "not found")", status: 1)
        }
        peripheral.discoverCharacteristics([characteristicID], for: service)
    }

    func peripheral(_ peripheral: CBPeripheral, didDiscoverCharacteristicsFor service: CBService, error: Error?) {
        guard error == nil,
              let characteristic = service.characteristics?.first(where: { $0.uuid == characteristicID }) else {
            finish("FFE1 characteristic unavailable: \(error?.localizedDescription ?? "not found")", status: 1)
        }
        channel = characteristic
        peripheral.setNotifyValue(true, for: characteristic)
    }

    func peripheral(_ peripheral: CBPeripheral, didUpdateNotificationStateFor characteristic: CBCharacteristic, error: Error?) {
        guard error == nil, characteristic.isNotifying else {
            finish("notifications unavailable: \(error?.localizedDescription ?? "not enabled")", status: 1)
        }
        sendNext()
    }

    func peripheral(_ peripheral: CBPeripheral, didUpdateValueFor characteristic: CBCharacteristic, error: Error?) {
        guard error == nil, let data = characteristic.value else {
            finish("notification failed: \(error?.localizedDescription ?? "empty value")", status: 1)
        }
        response.append(data)
        print("rx        \(hex(data))  \(String(decoding: data, as: UTF8.self).debugDescription)")
        guard String(decoding: response, as: UTF8.self).contains(">") else { return }

        if commandIndex == commands.count - 1 {
            decodeRPM()
            return
        }
        commandIndex += 1
        sendNext()
    }

    private func sendNext() {
        guard let adapter, let channel else { return }
        response.removeAll(keepingCapacity: true)
        let data = Data(commands[commandIndex].utf8)
        let type: CBCharacteristicWriteType = channel.properties.contains(.writeWithoutResponse) ? .withoutResponse : .withResponse
        print(commandIndex == commands.count - 1 ? "semantic  engine.rpm" : "command   \(commands[commandIndex].trimmingCharacters(in: .whitespacesAndNewlines))")
        print("tx        \(hex(data))")
        adapter.writeValue(data, for: channel, type: type)
    }

    private func decodeRPM() {
        let compact = String(decoding: response, as: UTF8.self)
            .uppercased()
            .filter { $0.isHexDigit }
        guard let marker = compact.range(of: "410C"), compact.distance(from: marker.upperBound, to: compact.endIndex) >= 4 else {
            finish("RPM response not found in \(String(decoding: response, as: UTF8.self).debugDescription)", status: 1)
        }
        let value = compact[marker.upperBound...].prefix(4)
        guard let raw = UInt16(value, radix: 16) else {
            finish("invalid RPM response", status: 1)
        }
        print("decoded   \(Double(raw) / 4.0) rpm")
        finish("done", status: 0)
    }

    private func finish(_ message: String, status: Int32) -> Never {
        if let adapter { central?.cancelPeripheralConnection(adapter) }
        print(message)
        exit(status)
    }

    private func hex(_ data: Data) -> String {
        data.map { String(format: "%02X", $0) }.joined(separator: " ")
    }
}

let probe = CarlyProbe()
withExtendedLifetime(probe) { RunLoop.main.run() }
