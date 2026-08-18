"""列出 HuggingFace 模型文件（经代理）。"""
import json
import sys
import urllib.request

proxy = urllib.request.ProxyHandler({"http": "http://127.0.0.1:10808", "https": "http://127.0.0.1:10808"})
opener = urllib.request.build_opener(proxy)


def list_repo(repo: str) -> None:
    url = f"https://huggingface.co/api/models/{repo}/tree/main"
    with opener.open(url, timeout=30) as r:
        files = json.load(r)
    print(f"--- {repo} ---")
    for f in files:
        size = f.get("size", 0) / 1e6
        print(f"  {f['path']:48s} {size:8.1f} MB")


if __name__ == "__main__":
    for repo in sys.argv[1:]:
        list_repo(repo)
