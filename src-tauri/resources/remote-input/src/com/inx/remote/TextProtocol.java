package com.inx.remote;

import java.io.DataInputStream;
import java.io.IOException;
import java.io.InputStream;
import java.util.Arrays;

/** 工作台与短生命周期输入程序之间的二进制协议；不包含日志或文件接口。 */
public final class TextProtocol {
    public static final int MAGIC = 0x494e5831;
    public static final long MAX_TTL_MS = 4000;
    public static final int SET = 1;
    public static final int CLEAR = 2;

    public static final class Request {
        public final long issued;
        public final long expires;
        public final int operation;
        public final byte[] text;
        Request(long issued, long expires, int operation, byte[] text) {
            this.issued = issued;
            this.expires = expires;
            this.operation = operation;
            this.text = text;
        }
        public void clear() { Arrays.fill(text, (byte) 0); }
    }

    public interface Action { boolean perform(); }

    public static Request read(InputStream stream) throws IOException {
        DataInputStream input = new DataInputStream(stream);
        if (input.readInt() != MAGIC) throw new IOException("invalid request");
        long issued = input.readLong();
        long expires = input.readLong();
        int operation = input.readUnsignedByte();
        int length = input.readUnsignedShort();
        if (issued < 0 || expires <= issued || expires - issued > MAX_TTL_MS
                || (operation != SET && operation != CLEAR)
                || (operation == SET && (length < 1 || length > 128))
                || (operation == CLEAR && length != 0)) {
            throw new IOException("invalid request");
        }
        byte[] text = new byte[length];
        try {
            input.readFully(text);
            if (input.read() != -1) throw new IOException("invalid request");
            for (byte value : text) {
                if (value < 0x20 || value > 0x7e) throw new IOException("invalid request");
            }
            return new Request(issued, expires, operation, text);
        } catch (IOException error) {
            Arrays.fill(text, (byte) 0);
            throw new IOException("invalid request");
        }
    }

    public static boolean current(Request request, long elapsedRealtime) {
        // /proc/uptime只有百分之一秒精度。不得把设备重启后的旧截止点当作未来许可。
        return elapsedRealtime >= request.issued - 20 && elapsedRealtime <= request.expires
                && request.expires - elapsedRealtime <= MAX_TTL_MS + 20;
    }

    public static String apply(Request request, long now, Action action) {
        if (!current(request, now)) return "XX_TEXT_EXPIRED";
        return action.perform() ? "XX_TEXT_APPLIED" : "XX_TEXT_REJECTED";
    }
}
