"""仅操作带本次验收标记的智能屏隔离库，不接受任意SQL。"""
import json
import re
import sys
from pathlib import Path

import pymysql

request = json.load(sys.stdin)
business, shared = request["business"], request["shared"]
match = re.fullmatch(r"inxaiot_desk_buddy_ui_b_([a-f0-9]{32})", business)
if not match or shared != "inxaiot_desk_buddy_ui_w_" + match[1]:
    raise ValueError("隔离库名称不匹配")
root = Path(__file__).resolve().parents[2]
description = (root / "test/测试数据说明.txt").read_text(encoding="utf-8-sig")
def value(label):
    return next(line.strip()[len(label):].strip() for line in description.splitlines() if line.strip().startswith(label))
username, password = value("平台数据库账号密码：").split("/", 1)
connection = pymysql.connect(host=value("平台主机："), port=3306, user=username, password=password, autocommit=True)
try:
    with connection.cursor() as cursor:
        for schema in [business, shared]:
            cursor.execute(f"SELECT nonce FROM `{schema}`.codex_screen_fixture")
            if cursor.fetchone() != (match[1],):
                raise ValueError("隔离库标记不匹配")
        action = request["action"]
        if action == "reject_shared":
            cursor.execute(f"CREATE TRIGGER `{shared}`.step7_reject_result BEFORE UPDATE ON `{shared}`.operation_target_result FOR EACH ROW SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT='step7 shared result failure'")
            result = {"ok": True}
        elif action == "restore_shared":
            cursor.execute(f"DROP TRIGGER IF EXISTS `{shared}`.step7_reject_result")
            result = {"ok": True}
        elif action == "summary":
            cursor.execute(f"SELECT CAST(id AS CHAR),name,app_version,version FROM `{business}`.smart_terminal_screen ORDER BY id")
            screens = cursor.fetchall()
            cursor.execute(f"SELECT id,operation_type,state,instance_id FROM `{shared}`.operation_record ORDER BY id")
            operations = cursor.fetchall()
            cursor.execute(f"SELECT COUNT(*) FROM `{shared}`.resource_lease WHERE lease_state='active' AND expires_at>UTC_TIMESTAMP(6)")
            result = {"screens": screens, "operations": operations, "activeLeases": cursor.fetchone()[0]}
        else:
            raise ValueError("不支持的验收动作")
        print(json.dumps(result, ensure_ascii=False))
finally:
    connection.close()
