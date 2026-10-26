package com.inx.remote;

import java.io.ByteArrayInputStream;
import java.io.ByteArrayOutputStream;
import java.io.DataOutputStream;
import java.io.IOException;
import java.util.Arrays;

public final class TextProtocolTest {
    private static byte[] packet(long issued, long expires, int op, byte[] text) throws Exception {
        ByteArrayOutputStream buffer = new ByteArrayOutputStream();
        DataOutputStream out = new DataOutputStream(buffer);
        out.writeInt(TextProtocol.MAGIC); out.writeLong(issued); out.writeLong(expires);
        out.writeByte(op); out.writeShort(text.length); out.write(text);
        return buffer.toByteArray();
    }
    private static void reject(byte[] packet) throws Exception {
        try { TextProtocol.read(new ByteArrayInputStream(packet)); throw new AssertionError("accepted invalid packet"); }
        catch (IOException expected) { }
    }
    public static void main(String[] args) throws Exception {
        byte[] text = "probe_%s'& \"$();".getBytes("US-ASCII");
        TextProtocol.Request request = TextProtocol.read(new ByteArrayInputStream(packet(10000, 14000, 1, text)));
        if (!Arrays.equals(text, request.text)) throw new AssertionError("payload changed");
        final int[] calls = {0};
        TextProtocol.Action action = new TextProtocol.Action() { public boolean perform() { calls[0]++; return true; } };
        if (!TextProtocol.apply(request, 14001, action).equals("XX_TEXT_EXPIRED") || calls[0] != 0) throw new AssertionError("late input");
        if (!TextProtocol.apply(request, 500, action).equals("XX_TEXT_EXPIRED") || calls[0] != 0) throw new AssertionError("old boot input");
        if (!TextProtocol.apply(request, 12000, action).equals("XX_TEXT_APPLIED") || calls[0] != 1) throw new AssertionError("valid input rejected");
        request.clear();
        if (!Arrays.equals(request.text, new byte[text.length])) throw new AssertionError("payload retained");
        TextProtocol.Request clear = TextProtocol.read(new ByteArrayInputStream(packet(100, 4000, 2, new byte[0])));
        if (clear.operation != TextProtocol.CLEAR) throw new AssertionError("clear operation");
        reject(packet(0, 10000, 1, text));
        reject(packet(-1, 100, 1, text));
        reject(packet(0, 4000, 9, text));
        reject(packet(0, 4000, 2, text));
        reject(packet(0, 4000, 1, new byte[0]));
        reject(packet(0, 4000, 1, new byte[129]));
        reject(packet(0, 4000, 1, "line\nother".getBytes("US-ASCII")));
        reject(packet(0, 4000, 1, "中文".getBytes("UTF-8")));
        byte[] valid = packet(0, 4000, 1, text);
        reject(Arrays.copyOf(valid, valid.length - 1));
        reject(Arrays.copyOf(valid, valid.length + 1));
        System.out.println("TextProtocol: 15 assertions passed");
    }
}
