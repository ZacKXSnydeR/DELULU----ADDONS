"""Verify: use the auto-issued token to actually get play links for The Godfather"""
import urllib.request
import json
import http.cookiejar
import uuid as uuid_lib

UA = "Mozilla/5.0 (Linux; Android 10; K) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Mobile Safari/537.36"
PLAYER = "https://123movienow.cc"

# Step 1: Generate UUID
our_uuid = str(uuid_lib.uuid4())
print(f"[1] Generated UUID: {our_uuid}")

# Step 2: Hit play endpoint with uuid cookie to get token
print("\n[2] Hitting play endpoint to harvest token...")
init_url = f"{PLAYER}/wefeed-h5api-bff/subject/play?subjectId=0&se=0&ep=0&detailPath=init"
req = urllib.request.Request(init_url, headers={
    "User-Agent": UA,
    "Accept": "application/json",
    "Cookie": f"uuid={our_uuid}",
    "Referer": f"{PLAYER}/",
    "Origin": PLAYER,
})
resp = urllib.request.urlopen(req, timeout=15)
body = resp.read().decode()
print(f"Response: {body[:200]}")

# Extract token from Set-Cookie
token = None
for header in resp.headers.get_all("set-cookie") or []:
    if header.startswith("token="):
        token = header.split("token=")[1].split(";")[0]
        break

if token:
    print(f"\n[OK] Got fresh token: {token[:60]}...")
    # Decode JWT to see expiry
    import base64
    payload = json.loads(base64.b64decode(token.split(".")[1] + "=="))
    import datetime
    exp = datetime.datetime.fromtimestamp(payload["exp"])
    print(f"     Token expires: {exp}")
    print(f"     UID: {payload.get('uid')}")
else:
    print("\n[FAIL] No token in Set-Cookie")
    exit(1)

# Step 3: Now use this token+uuid to get REAL play links for The Godfather
# First we need the real subjectId — search moviebox.pk
import re
print("\n[3] Searching MovieBox for The Godfather...")
search_url = f"https://moviebox.pk/web/searchResult?keyword=the-godfather"
search_req = urllib.request.Request(search_url, headers={"User-Agent": UA, "Accept": "text/html,*/*"})
html = urllib.request.urlopen(search_req, timeout=15).read().decode("utf-8", "ignore")
data_match = re.search(r'id="__NUXT_DATA__"[^>]*>(.*?)</script>', html, re.S)
if data_match:
    raw = json.loads(data_match.group(1))
    for item in raw:
        if isinstance(item, dict) and "subjectId" in item and "title" in item:
            title = raw[item["title"]] if isinstance(item["title"], int) else item["title"]
            sid = raw[item["subjectId"]] if isinstance(item["subjectId"], int) else item["subjectId"]
            dp = raw[item.get("detailPath", "")] if isinstance(item.get("detailPath", ""), int) else item.get("detailPath", "")
            if "godfather" in str(title).lower():
                print(f"   Found: '{title}' subjectId={sid} detailPath={dp}")

                # Step 4: Hit play API with our fresh credentials
                print(f"\n[4] Fetching play links with fresh token...")
                play_url = f"{PLAYER}/wefeed-h5api-bff/subject/play?subjectId={sid}&se=0&ep=0&detailPath={dp}"
                play_req = urllib.request.Request(play_url, headers={
                    "User-Agent": UA,
                    "Accept": "application/json",
                    "Cookie": f"uuid={our_uuid}; token={token}",
                    "Referer": f"{PLAYER}/spa/videoPlayPage/movies/{dp}",
                    "x-client-info": json.dumps({"timezone": "Asia/Dhaka"}),
                })
                play_resp = urllib.request.urlopen(play_req, timeout=15)
                play_data = json.loads(play_resp.read().decode())
                
                print(f"    code: {play_data.get('code')}")
                data = play_data.get("data", {})
                streams = data.get("streams", [])
                hls = data.get("hls", [])
                dash = data.get("dash", [])
                print(f"    streams: {len(streams)}, hls: {len(hls)}, dash: {len(dash)}")
                print(f"    freeNum: {data.get('freeNum')}")
                
                for s in streams[:5]:
                    print(f"    → {s.get('resolutions')}p: {str(s.get('url', ''))[:80]}...")
                
                break
