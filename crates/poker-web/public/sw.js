"use strict";

// IMPORTANT: bump this version string on every deploy to invalidate old caches.
// Consider automating this as part of your build/deploy pipeline.
var CACHE_VERSION = "v2";
var CACHE_NAME = "poker-" + CACHE_VERSION;

var PRECACHE_URLS = [
  "/poker/"
];

// Install: precache essential assets and immediately activate
self.addEventListener("install", function (event) {
  event.waitUntil(
    caches
      .open(CACHE_NAME)
      .then(function (cache) {
        return cache.addAll(PRECACHE_URLS);
      })
      .then(function () {
        // Skip waiting so the new SW activates immediately
        return self.skipWaiting();
      })
  );
});

// Activate: purge all old caches and claim clients immediately
self.addEventListener("activate", function (event) {
  event.waitUntil(
    caches
      .keys()
      .then(function (keys) {
        return Promise.all(
          keys
            .filter(function (key) {
              return key !== CACHE_NAME;
            })
            .map(function (key) {
              return caches.delete(key);
            })
        );
      })
      .then(function () {
        // Take control of all open clients without requiring a reload
        return self.clients.claim();
      })
  );
});

// Fetch: network-first strategy — only fall back to cache when offline
self.addEventListener("fetch", function (event) {
  if (event.request.method !== "GET") {
    return;
  }

  // Skip caching for WebSocket upgrade requests and chrome-extension URLs
  if (
    event.request.url.startsWith("chrome-extension://") ||
    event.request.headers.get("Upgrade") === "websocket"
  ) {
    return;
  }

  event.respondWith(
    fetch(event.request)
      .then(function (response) {
        // Only cache successful responses
        if (response.ok) {
          var cacheCopy = response.clone();
          caches.open(CACHE_NAME).then(function (cache) {
            cache.put(event.request, cacheCopy);
          });
        }
        return response;
      })
      .catch(function () {
        // Network failed — try the cache
        return caches.match(event.request).then(function (cached) {
          return (
            cached ||
            new Response("<h1>Service Unavailable</h1>", {
              status: 503,
              statusText: "Service Unavailable",
              headers: new Headers({
                "Content-Type": "text/html",
              }),
            })
          );
        });
      })
  );
});
