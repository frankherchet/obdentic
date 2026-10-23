package de.obdentic.app;

import android.bluetooth.*;
import android.content.Context;
import android.os.*;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.util.UUID;

/** One GATT connection; never call exchange on the callback thread. */
final class BleTransport implements NativeCore.Exchange {
    static final UUID SERVICE = UUID.fromString("0000ffe0-0000-1000-8000-00805f9b34fb");
    static final UUID CHANNEL = UUID.fromString("0000ffe1-0000-1000-8000-00805f9b34fb");
    private static final UUID CCC = UUID.fromString("00002902-0000-1000-8000-00805f9b34fb");
    interface Log { void line(String direction, String text); }
    private final Object lock = new Object();
    private final HandlerThread thread = new HandlerThread("obd-gatt");
    private final Log log;
    private BluetoothGatt gatt;
    private BluetoothGattCharacteristic channel;
    private boolean ready, closed, writeDone;
    private String failure;
    private final StringBuilder received = new StringBuilder();
    private boolean waiting;
    BleTransport(Log log) { this.log = log; thread.start(); }
    void connect(Context context, BluetoothDevice device) throws Exception {
        synchronized (lock) {
            try { gatt = device.connectGatt(context, false, callbacks, BluetoothDevice.TRANSPORT_LE,
                    BluetoothDevice.PHY_LE_1M_MASK, new Handler(thread.getLooper())); }
            catch(SecurityException e) { throw new IOException("Bluetooth permission missing",e); }
            await(() -> ready, 20000, "BLE connection timed out");
        }
    }
    private interface Condition { boolean yes(); }
    private void await(Condition condition, long timeout, String message) throws Exception {
        long until = SystemClock.elapsedRealtime() + timeout;
        while (!condition.yes()) {
            if (closed) throw new IOException("Disconnected");
            if (failure != null) throw new IOException(failure);
            long left = until - SystemClock.elapsedRealtime();
            if (left <= 0) throw new IOException(message);
            lock.wait(left);
        }
    }
    @SuppressWarnings("deprecation")
    public String exchange(String command, long timeoutMs) throws Exception {
        synchronized (lock) {
            if (!ready || closed) throw new IOException("Adapter disconnected");
            received.setLength(0); waiting = true;
            log.line("TX", command);
            long until = SystemClock.elapsedRealtime() + Math.max(1, Math.min(timeoutMs, 60000));
            try {
                byte[] bytes = command.getBytes(StandardCharsets.US_ASCII);
                for (int offset = 0; offset < bytes.length; offset += 20) {
                    byte[] part = java.util.Arrays.copyOfRange(bytes, offset, Math.min(offset + 20, bytes.length));
                    writeDone = false;
                    channel.setWriteType((channel.getProperties() & BluetoothGattCharacteristic.PROPERTY_WRITE) != 0
                            ? BluetoothGattCharacteristic.WRITE_TYPE_DEFAULT : BluetoothGattCharacteristic.WRITE_TYPE_NO_RESPONSE);
                    channel.setValue(part);
                    if (!gatt.writeCharacteristic(channel)) throw new IOException("BLE write rejected");
                    await(() -> writeDone, Math.max(1, until - SystemClock.elapsedRealtime()), "BLE write timed out");
                }
                await(() -> received.indexOf(">") >= 0, Math.max(1, until - SystemClock.elapsedRealtime()), "Adapter response timed out");
                return received.substring(0, received.indexOf(">") + 1);
            } catch (SecurityException e) {
                failure = "Bluetooth permission missing"; close(); throw new IOException(failure,e);
            } catch (Exception e) {
                failure = e.getMessage();
                // A late prompt cannot safely be associated with the next command.
                close();
                throw e;
            } finally { waiting = false; }
        }
    }
    @SuppressWarnings("deprecation")
    private final BluetoothGattCallback callbacks = new BluetoothGattCallback() {
        public void onConnectionStateChange(BluetoothGatt g, int status, int state) {
            try { synchronized (lock) {
                if (closed) return;
                if (status != BluetoothGatt.GATT_SUCCESS || state == BluetoothProfile.STATE_DISCONNECTED) {
                    failure = "BLE disconnected (" + status + ")"; ready = false; lock.notifyAll();
                } else if (state == BluetoothProfile.STATE_CONNECTED && !g.discoverServices()) {
                    failure = "BLE service discovery rejected"; lock.notifyAll();
                }
            } } catch(SecurityException e) { permissionFailed(); }
        }
        public void onServicesDiscovered(BluetoothGatt g, int status) {
            try { synchronized (lock) {
                if (closed) return;
                BluetoothGattService service = g.getService(SERVICE);
                channel = service == null ? null : service.getCharacteristic(CHANNEL);
                if (status != BluetoothGatt.GATT_SUCCESS || channel == null ||
                    (channel.getProperties() & BluetoothGattCharacteristic.PROPERTY_NOTIFY) == 0 ||
                    (channel.getProperties() & (BluetoothGattCharacteristic.PROPERTY_WRITE | BluetoothGattCharacteristic.PROPERTY_WRITE_NO_RESPONSE)) == 0) {
                    failure = "Carly FFE0/FFE1 notify/write service missing"; lock.notifyAll(); return;
                }
                BluetoothGattDescriptor descriptor = channel.getDescriptor(CCC);
                if (descriptor == null || !g.setCharacteristicNotification(channel, true)) {
                    failure = "Carly notifications unavailable"; lock.notifyAll(); return;
                }
                descriptor.setValue(BluetoothGattDescriptor.ENABLE_NOTIFICATION_VALUE);
                if (!g.writeDescriptor(descriptor)) { failure = "Notification setup rejected"; lock.notifyAll(); }
            } } catch(SecurityException e) { permissionFailed(); }
        }
        public void onDescriptorWrite(BluetoothGatt g, BluetoothGattDescriptor descriptor, int status) {
            synchronized (lock) { if (closed) return; ready = status == BluetoothGatt.GATT_SUCCESS;
                if (!ready) failure = "Notification setup failed (" + status + ")"; lock.notifyAll(); }
        }
        public void onCharacteristicWrite(BluetoothGatt g, BluetoothGattCharacteristic c, int status) {
            synchronized (lock) { if (closed) return; writeDone = status == BluetoothGatt.GATT_SUCCESS;
                if (!writeDone) failure = "BLE write failed (" + status + ")"; lock.notifyAll(); }
        }
        public void onCharacteristicChanged(BluetoothGatt g, BluetoothGattCharacteristic c) { accept(c.getValue()); }
        public void onCharacteristicChanged(BluetoothGatt g, BluetoothGattCharacteristic c, byte[] value) { accept(value); }
    };
    private void permissionFailed() { synchronized(lock) { failure="Bluetooth permission missing"; ready=false; lock.notifyAll(); } }
    private void accept(byte[] bytes) {
        if (bytes == null) return;
        String text = new String(bytes, StandardCharsets.US_ASCII);
        log.line("RX", text);
        synchronized (lock) {
            if (!waiting || closed) return;
            if (received.length() + text.length() > 8192) failure = "Adapter response exceeds 8 KiB limit";
            else received.append(text);
            lock.notifyAll();
        }
    }
    boolean isOpen() { synchronized(lock) { return ready && !closed && failure == null; } }
    void close() {
        synchronized (lock) {
            if (closed) return;
            closed = true; ready = false; lock.notifyAll();
            if (gatt != null) { try { gatt.disconnect(); } catch (SecurityException ignored) {} try { gatt.close(); } catch(SecurityException ignored) {} gatt = null; }
        }
        thread.quitSafely();
    }
}
