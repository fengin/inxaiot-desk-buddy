package com.inx.remote;

import android.graphics.Rect;
import android.os.Bundle;
import android.os.SystemClock;
import android.view.accessibility.AccessibilityNodeInfo;
import com.android.uiautomator.core.Configurator;
import com.android.uiautomator.core.UiObject;
import com.android.uiautomator.core.UiSelector;
import com.android.uiautomator.testrunner.UiAutomatorTestCase;

/** 仅操作小新已聚焦的可编辑节点。正文不进入参数、剪贴板、磁盘或日志。 */
public final class FocusedText extends UiAutomatorTestCase {
    private static final String PACKAGE = "chat.xiaoxin.app";
    private static final class FocusedObject extends UiObject {
        FocusedObject(int instance) {
            super(new UiSelector().packageName(PACKAGE).focused(true).enabled(true).instance(instance));
        }
        AccessibilityNodeInfo node() { return findAccessibilityNodeInfo(0); }
    }

    public void testSetFocusedText() {
        TextProtocol.Request request = null;
        AccessibilityNodeInfo chosen = null;
        String result = "XX_TEXT_ERROR";
        Thread watchdog = new Thread(new Runnable() {
            public void run() {
                try { Thread.sleep(12000); Runtime.getRuntime().halt(2); }
                catch (InterruptedException ignored) { }
            }
        }, "inx-input-deadline");
        watchdog.setDaemon(true);
        watchdog.start();
        try {
            request = TextProtocol.read(System.in);
            Configurator.getInstance().setWaitForIdleTimeout(150).setWaitForSelectorTimeout(0);
            boolean ambiguous = false;
            while (TextProtocol.current(request, SystemClock.elapsedRealtime())) {
                for (int index = 0; index < 16; index++) {
                    AccessibilityNodeInfo candidate = new FocusedObject(index).node();
                    if (candidate == null) break;
                    if (acceptable(candidate)) {
                        if (chosen != null) { candidate.recycle(); ambiguous = true; break; }
                        chosen = candidate;
                    } else { candidate.recycle(); }
                }
                if (chosen != null || ambiguous) break;
                SystemClock.sleep(100);
            }
            if (ambiguous) {
                result = "XX_TEXT_AMBIGUOUS";
            } else if (!TextProtocol.current(request, SystemClock.elapsedRealtime())) {
                result = "XX_TEXT_EXPIRED";
            } else if (chosen == null) {
                result = "XX_TEXT_NOT_READY";
            } else if (!chosen.refresh() || !acceptable(chosen)) {
                result = "XX_TEXT_FOCUS_CHANGED";
            } else {
                final AccessibilityNodeInfo target = chosen;
                final Bundle arguments = new Bundle();
                arguments.putCharSequence(AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE,
                        request.operation == TextProtocol.CLEAR ? "" : new String(request.text, "US-ASCII"));
                // 最后一次到期检查紧贴系统调用；系统已接收的动作不承诺可以撤销。
                result = TextProtocol.apply(request, SystemClock.elapsedRealtime(), new TextProtocol.Action() {
                    public boolean perform() {
                        return target.performAction(AccessibilityNodeInfo.ACTION_SET_TEXT, arguments);
                    }
                });
                arguments.clear();
            }
        } catch (Throwable ignored) {
            result = "XX_TEXT_ERROR";
        } finally {
            watchdog.interrupt();
            if (request != null) request.clear();
            if (chosen != null) chosen.recycle();
            System.out.println(result);
        }
    }

    private static boolean acceptable(AccessibilityNodeInfo node) {
        Rect bounds = new Rect();
        node.getBoundsInScreen(bounds);
        return PACKAGE.contentEquals(node.getPackageName() == null ? "" : node.getPackageName())
                && node.isFocused() && node.isEditable() && node.isEnabled()
                && node.isVisibleToUser() && !bounds.isEmpty()
                && (node.getActions() & AccessibilityNodeInfo.ACTION_SET_TEXT) != 0;
    }
}
