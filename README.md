# surl

a simple short url service

简易短网址服务

## 部署

```bash
SURL_WEBSITE='https://example.com/s' SURL_HOST='127.0.0.1' SURL_PORT=8080 ./surl
```

## 使用方法

```http
POST  http://example.com/new HTTP/1.1
Content-Type: application/x-www-form-urlencoded

url=https://www.google.com
```

你会收到一个短网址，如下：

```
{"url": "https://example.com/s/1"}
```
