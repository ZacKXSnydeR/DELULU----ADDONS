<div align="center">

# DELULU ADDONS REPOSITORY
### The Official High-Performance Native Binary Extension (NBE) Hub

[![Rust](https://img.shields.io/badge/Language-Rust-E34C26?logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![Architecture](https://img.shields.io/badge/Architecture-Native_Binary_IPC-7289DA)](https://github.com/DeluluStream/addons-repo)
[![License](https://img.shields.io/badge/License-MIT-007ACC)](LICENSE)
[![Status](https://img.shields.io/badge/Status-Production_Ready-4CAF50)](https://github.com/DeluluStream/addons-repo)

**Decentralized Media Resolution • Local-First Metadata Aggregation • High-Speed HLS Tunneling**

[Architecture](#-core-architecture) • [Addons Directory](#-official-addons) • [Installation](#-deployment--integration) • [Developer Guide](#-contribution--extension)

</div>

---

## 💎 Core Philosophy

The **Delulu Addons Repository** is a centralized monorepo hosting native extensions for the Delulu ecosystem. Our mission is to provide a high-performance, decentralized bridge between disparate media sources and the local user environment. 

By moving away from traditional web-based addon models, we prioritize **Native Execution**, **Privacy**, and **Zero-Latency** data processing.

---

## 🏗️ Core Architecture: The "NBE" Model

DeluluStream utilizes a proprietary **Native Binary Extension (NBE)** architecture. Instead of running scripts in a sandbox or making external HTTP calls to addon servers, Delulu executes native binaries directly on the host machine.

### The JSON-RPC over IPC Advantage
Communication occurs via a dedicated **STDIN/STDOUT Pipe** using a strict JSON-RPC 2.0 protocol.

| Feature | Traditional (HTTP/Web) | Delulu (Native IPC) |
| :--- | :--- | :--- |
| **Latency** | 50ms - 200ms (Network overhead) | **< 1ms (Memory-to-Memory)** |
| **Overhead** | High (TCP/IP, HTTP Headers) | **Minimal (Raw Byte Stream)** |
| **Port Conflicts** | Common (Needs 8080, 3000, etc.) | **None (Standard I/O Pipes)** |
| **Security** | Server-side tracking possible | **Local-only execution** |

---

## 📦 Official Addons

This repository maintains the "Gold Standard" resolvers for the Delulu ecosystem, built entirely in **Type-Safe Rust**.

### 🐊 EmbeGator: The Parallel Resolver
*High-concurrency stream discovery and manifest aggregation.*

- **Parallelized Manifest Discovery:** Scans decentralized media endpoints simultaneously using `Tokio`.
- **Heuristic Sorting:** Automatically ranks stream quality based on bitrates, codecs, and CDN response times.
- **Unified Metadata Schema:** Normalizes data from heterogeneous sources into a single, clean JSON object.
- **Low Footprint:** Written in Rust for minimal RAM usage even during heavy scraping tasks.

### 📦 MotherBox: The HLS Tunneling Engine
*Protocol bridging and secure network proxying.*

- **Embedded Reverse Proxy:** Features a built-in `Hyper`-powered proxy to handle complex manifest redirects.
- **HLS Tunneling Capability:** Securely tunnels HLS/M3U8 streams to the local player, bypassing traditional ISP-level network restrictions or geoblocks.
- **Dynamic Header Injection:** Automatically manages `Referer`, `User-Agent`, and `Cookie-` headers for seamless CDN interaction.
- **Multi-Track Extraction:** Real-time extraction of multiple audio languages and subtitle tracks from raw media manifests.

---

## 🚀 Deployment & Integration

Integrating this repository into your local Delulu instance takes seconds.

### 1. Locate the Catalog
The `catalog.json` file in this repository acts as the manifest for all available binaries. 

### 2. Synchronization
Paste the **Raw URL** of the catalog into the Delulu Addon Manager:
```text
https://raw.githubusercontent.com/[USER]/addons-repo/main/catalog.json
```

### 3. Native Optimization
Delulu will automatically pull the optimized binary for your specific architecture (Windows x64, Linux x86_64, etc.), ensuring maximum hardware utilization.

---

## 🛠️ Contribution & Extension

We welcome contributions to the NBE ecosystem. Whether you are building a new resolver or optimizing a proxy engine:

1. **Clone the Repo:** Ensure you have the Rust toolchain installed.
2. **Follow the Spec:** All addons must implement the standard `init`, `search`, and `resolve` JSON-RPC methods.
3. **Test Local IPC:** Use `cargo run` and pipe JSON inputs manually to verify STDOUT responses.

---

## ⚖️ Legal Disclaimer

**DELULU ADDONS** is an open-source technical framework designed for decentralized media resolution and educational networking research.

- **Neutrality:** This software acts as a neutral technical bridge. It does not host, provide, or index any copyrighted content.
- **User Responsibility:** Users are solely responsible for the links they provide to the resolver and must ensure compliance with their local jurisdiction and the terms of service of third-party CDN providers.
- **Tooling Intent:** These tools are intended for researchers and developers exploring high-performance IPC and HLS proxying techniques. 
- **No Liability:** The developers and contributors assume no liability for any misuse of the software or data processed through these local-first tools.

---

<div align="center">
    <sub>Built with precision by the Delulu Community.</sub>
</div>
