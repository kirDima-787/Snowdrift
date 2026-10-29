#!/usr/bin/env python3
"""
Fetch download statistics for all Snowdrift GitHub releases.
"""
import json
import subprocess
import sys
import urllib.request

if hasattr(sys.stdout, "reconfigure"):
    sys.stdout.reconfigure(encoding="utf-8")

REPO = "kirDima-787/Snowdrift"

def get_token():
    try:
        p = subprocess.Popen(["git", "credential", "fill"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
        out, _ = p.communicate("protocol=https\nhost=github.com\n\n")
        creds = dict(line.split("=", 1) for line in out.strip().splitlines() if "=" in line)
        return creds.get("password")
    except Exception:
        return None

def main():
    token = get_token()
    headers = {"User-Agent": "Snowdrift-Stats-Tool", "Accept": "application/vnd.github.v3+json"}
    if token:
        headers["Authorization"] = f"token {token}"

    req = urllib.request.Request(f"https://api.github.com/repos/{REPO}/releases", headers=headers)
    with urllib.request.urlopen(req) as resp:
        releases = json.loads(resp.read())

    total = 0
    per_file = {}
    print(f"Статистика скачиваний для {REPO}:\n" + "=" * 45)

    for r in releases:
        tag = r.get("tag_name")
        r_sum = 0
        items = []
        for a in r.get("assets", []):
            name = a.get("name")
            cnt = a.get("download_count", 0)
            r_sum += cnt
            total += cnt
            per_file[name] = per_file.get(name, 0) + cnt
            items.append(f"    - {name}: {cnt}")
        print(f"Релиз {tag} (всего {r_sum}):")
        for item in items:
            print(item)

    print("=" * 45)
    print(f"Всего скачиваний по всем релизам: {total}\n")
    print("По типам файлов:")
    for name, cnt in sorted(per_file.items(), key=lambda x: -x[1]):
        print(f"  - {name}: {cnt}")

    if token:
        try:
            req_c = urllib.request.Request(f"https://api.github.com/repos/{REPO}/traffic/clones", headers=headers)
            with urllib.request.urlopen(req_c) as resp:
                clones = json.loads(resp.read())
                print(f"\nКлонирований репозитория (14 дней): {clones.get('count')} (уникальных: {clones.get('uniques')})")
        except Exception:
            pass

if __name__ == "__main__":
    main()
