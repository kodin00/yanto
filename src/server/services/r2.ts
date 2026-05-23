import crypto from "node:crypto";
import fs from "node:fs/promises";
import path from "node:path";
import { getStoredR2Settings } from "./settings.js";

type UploadInput = {
  filePath: string;
  filename: string;
  contentType?: string;
};

function hmac(key: crypto.BinaryLike, value: string) {
  return crypto.createHmac("sha256", key).update(value).digest();
}

function sha256(value: Buffer | string) {
  return crypto.createHash("sha256").update(value).digest("hex");
}

function amzDateParts(date = new Date()) {
  const iso = date.toISOString().replace(/[:-]|\.\d{3}/g, "");
  return { amzDate: iso, dateStamp: iso.slice(0, 8) };
}

function encodeKey(key: string) {
  return key
    .split("/")
    .filter(Boolean)
    .map((part) => encodeURIComponent(part))
    .join("/");
}

function safePrefix(input: string) {
  return input
    .split("/")
    .map((part) =>
      part
        .trim()
        .toLowerCase()
        .replace(/[^a-z0-9._-]+/g, "-")
        .replace(/^-+|-+$/g, "")
    )
    .filter(Boolean)
    .join("/");
}

export async function uploadFileToR2(input: UploadInput) {
  const settings = await getStoredR2Settings();
  if (!settings.enabled) {
    throw new Error("Cloudflare R2 uploads are disabled in settings.");
  }
  if (!settings.accountId || !settings.bucket || !settings.accessKeyId || !settings.secretAccessKey) {
    throw new Error("Cloudflare R2 settings are incomplete.");
  }

  const body = await fs.readFile(input.filePath);
  const host = `${settings.accountId}.r2.cloudflarestorage.com`;
  const objectKey = [safePrefix(settings.prefix), input.filename].filter(Boolean).join("/");
  const canonicalUri = `/${encodeURIComponent(settings.bucket)}/${encodeKey(objectKey)}`;
  const url = `https://${host}${canonicalUri}`;
  const { amzDate, dateStamp } = amzDateParts();
  const payloadHash = sha256(body);
  const canonicalHeaders = `host:${host}\nx-amz-content-sha256:${payloadHash}\nx-amz-date:${amzDate}\n`;
  const signedHeaders = "host;x-amz-content-sha256;x-amz-date";
  const canonicalRequest = ["PUT", canonicalUri, "", canonicalHeaders, signedHeaders, payloadHash].join("\n");
  const credentialScope = `${dateStamp}/auto/s3/aws4_request`;
  const stringToSign = ["AWS4-HMAC-SHA256", amzDate, credentialScope, sha256(canonicalRequest)].join("\n");
  const signingKey = hmac(hmac(hmac(hmac(`AWS4${settings.secretAccessKey}`, dateStamp), "auto"), "s3"), "aws4_request");
  const signature = crypto.createHmac("sha256", signingKey).update(stringToSign).digest("hex");

  const response = await fetch(url, {
    method: "PUT",
    headers: {
      Authorization: `AWS4-HMAC-SHA256 Credential=${settings.accessKeyId}/${credentialScope}, SignedHeaders=${signedHeaders}, Signature=${signature}`,
      "Content-Type": input.contentType ?? "application/octet-stream",
      "x-amz-content-sha256": payloadHash,
      "x-amz-date": amzDate
    },
    body
  });

  if (!response.ok) {
    const message = await response.text().catch(() => response.statusText);
    throw new Error(`Cloudflare R2 upload failed: ${message || response.statusText}`);
  }

  return {
    bucket: settings.bucket,
    key: objectKey,
    endpoint: `https://${host}`,
    filename: path.basename(input.filename),
    size: body.byteLength
  };
}
