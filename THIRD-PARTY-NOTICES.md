# Third-party notices

pingtable is built from the open-source components listed here. Each is used
under the license shown; where a component offers a choice, pingtable uses it
under the Apache License 2.0 or the MIT License. The full license texts are in
each component's source, linked below.

## Microsoft Edge WebView2 loader

The Windows package includes `WebView2Loader.dll`, part of the Microsoft Edge
WebView2 SDK, Copyright (C) Microsoft Corporation. It is redistributed unmodified
under the license of that SDK: <https://www.nuget.org/packages/Microsoft.Web.WebView2>

## Rust crates (108, for the `x86_64-pc-windows-gnu` build)

| Crate | Version | License | Source |
|---|---|---|---|
| ascii | 1.1.0 | Apache-2.0 OR MIT | <https://github.com/tomprogrammer/rust-ascii> |
| bitflags | 2.13.2 | MIT OR Apache-2.0 | <https://github.com/bitflags/bitflags> |
| bytes | 1.12.1 | MIT | <https://github.com/tokio-rs/bytes> |
| cc | 1.11.2 | MIT OR Apache-2.0 | <https://github.com/rust-lang/cc-rs> |
| cfg-if | 1.0.5 | MIT OR Apache-2.0 | <https://github.com/rust-lang/cfg-if> |
| chunked_transfer | 1.5.0 | MIT OR Apache-2.0 | <https://github.com/frewsxcv/rust-chunked-transfer> |
| cookie | 0.18.2 | MIT OR Apache-2.0 | <https://github.com/SergioBenitez/cookie-rs> |
| crossbeam-channel | 0.5.17 | MIT OR Apache-2.0 | <https://github.com/crossbeam-rs/crossbeam> |
| crossbeam-utils | 0.8.23 | MIT OR Apache-2.0 | <https://github.com/crossbeam-rs/crossbeam> |
| deranged | 0.5.9 | MIT OR Apache-2.0 | <https://github.com/jhpratt/deranged> |
| displaydoc | 0.2.7 | MIT OR Apache-2.0 | <https://github.com/yaahc/displaydoc> |
| dns-lookup | 4.0.2 | MIT OR Apache-2.0 | <https://github.com/keeperofdakeys/dns-lookup/> |
| dpi | 0.1.2 | Apache-2.0 AND MIT | <https://github.com/rust-windowing/winit> |
| dunce | 1.0.5 | CC0-1.0 OR MIT-0 OR Apache-2.0 | <https://gitlab.com/kornelski/dunce> |
| embed-resource | 3.0.12 | MIT | <https://github.com/nabijaczleweli/rust-embed-resource> |
| equivalent | 1.0.3 | Apache-2.0 OR MIT | <https://github.com/indexmap-rs/equivalent> |
| find-msvc-tools | 0.1.15 | MIT OR Apache-2.0 | <https://github.com/rust-lang/cc-rs> |
| form_urlencoded | 1.2.2 | MIT OR Apache-2.0 | <https://github.com/servo/rust-url> |
| getrandom | 0.4.3 | MIT OR Apache-2.0 | <https://github.com/rust-random/getrandom> |
| hashbrown | 0.17.1 | MIT OR Apache-2.0 | <https://github.com/rust-lang/hashbrown> |
| http | 1.5.0 | MIT OR Apache-2.0 | <https://github.com/hyperium/http> |
| httpdate | 1.0.3 | MIT OR Apache-2.0 | <https://github.com/pyfisch/httpdate> |
| icu_collections | 2.4.0 | Unicode-3.0 | <https://github.com/unicode-org/icu4x> |
| icu_locale_core | 2.4.0 | Unicode-3.0 | <https://github.com/unicode-org/icu4x> |
| icu_normalizer | 2.4.0 | Unicode-3.0 | <https://github.com/unicode-org/icu4x> |
| icu_normalizer_data | 2.4.0 | Unicode-3.0 | <https://github.com/unicode-org/icu4x> |
| icu_properties | 2.4.0 | Unicode-3.0 | <https://github.com/unicode-org/icu4x> |
| icu_properties_data | 2.4.0 | Unicode-3.0 | <https://github.com/unicode-org/icu4x> |
| icu_provider | 2.4.0 | Unicode-3.0 | <https://github.com/unicode-org/icu4x> |
| idna | 1.1.0 | MIT OR Apache-2.0 | <https://github.com/servo/rust-url/> |
| idna_adapter | 1.2.2 | Apache-2.0 OR MIT | <https://github.com/hsivonen/idna_adapter> |
| if-addrs | 0.15.0 | MIT OR BSD-3-Clause | <https://github.com/messense/if-addrs> |
| indexmap | 2.14.2 | Apache-2.0 OR MIT | <https://github.com/indexmap-rs/indexmap> |
| itoa | 1.0.19 | MIT OR Apache-2.0 | <https://github.com/dtolnay/itoa> |
| libc | 0.2.191 | MIT OR Apache-2.0 | <https://github.com/rust-lang/libc> |
| litemap | 0.8.3 | Unicode-3.0 | <https://github.com/unicode-org/icu4x> |
| lock_api | 0.4.14 | MIT OR Apache-2.0 | <https://github.com/Amanieu/parking_lot> |
| log | 0.4.35 | MIT OR Apache-2.0 | <https://github.com/rust-lang/log> |
| memchr | 2.8.3 | Unlicense OR MIT | <https://github.com/BurntSushi/memchr> |
| num-conv | 0.2.2 | MIT OR Apache-2.0 | <https://github.com/jhpratt/num-conv> |
| parking_lot | 0.12.5 | MIT OR Apache-2.0 | <https://github.com/Amanieu/parking_lot> |
| parking_lot_core | 0.9.12 | MIT OR Apache-2.0 | <https://github.com/Amanieu/parking_lot> |
| percent-encoding | 2.3.2 | MIT OR Apache-2.0 | <https://github.com/servo/rust-url/> |
| potential_utf | 0.1.6 | Unicode-3.0 | <https://github.com/unicode-org/icu4x> |
| powerfmt | 0.2.1 | MIT OR Apache-2.0 | <https://github.com/jhpratt/powerfmt> |
| proc-macro2 | 1.0.107 | MIT OR Apache-2.0 | <https://github.com/dtolnay/proc-macro2> |
| quote | 1.0.47 | MIT OR Apache-2.0 | <https://github.com/dtolnay/quote> |
| raw-window-handle | 0.6.2 | MIT OR Apache-2.0 OR Zlib | <https://github.com/rust-windowing/raw-window-handle> |
| rustc_version | 0.4.1 | MIT OR Apache-2.0 | <https://github.com/djc/rustc-version-rs> |
| scopeguard | 1.2.0 | MIT OR Apache-2.0 | <https://github.com/bluss/scopeguard> |
| semver | 1.0.28 | MIT OR Apache-2.0 | <https://github.com/dtolnay/semver> |
| serde | 1.0.229 | MIT OR Apache-2.0 | <https://github.com/serde-rs/serde> |
| serde_core | 1.0.229 | MIT OR Apache-2.0 | <https://github.com/serde-rs/serde> |
| serde_derive | 1.0.229 | MIT OR Apache-2.0 | <https://github.com/serde-rs/serde> |
| serde_json | 1.0.154 | MIT OR Apache-2.0 | <https://github.com/serde-rs/json> |
| serde_spanned | 1.1.3 | MIT OR Apache-2.0 | <https://github.com/toml-rs/toml> |
| shlex | 2.0.1 | MIT OR Apache-2.0 | <https://github.com/comex/rust-shlex> |
| smallvec | 1.16.5 | MIT OR Apache-2.0 | <https://github.com/servo/rust-smallvec> |
| socket2 | 0.6.5 | MIT OR Apache-2.0 | <https://github.com/rust-lang/socket2> |
| stable_deref_trait | 1.2.1 | MIT OR Apache-2.0 | <https://github.com/storyyeller/stable_deref_trait> |
| syn | 2.0.119 | MIT OR Apache-2.0 | <https://github.com/dtolnay/syn> |
| syn | 3.0.10 | MIT OR Apache-2.0 | <https://github.com/dtolnay/syn> |
| synstructure | 0.14.0 | MIT | <https://github.com/mystor/synstructure> |
| tao | 0.37.2 | Apache-2.0 | <https://github.com/tauri-apps/tao> |
| thiserror | 2.0.21 | MIT OR Apache-2.0 | <https://github.com/dtolnay/thiserror> |
| thiserror-impl | 2.0.21 | MIT OR Apache-2.0 | <https://github.com/dtolnay/thiserror> |
| time | 0.3.55 | MIT OR Apache-2.0 | <https://github.com/time-rs/time> |
| time-core | 0.1.9 | MIT OR Apache-2.0 | <https://github.com/time-rs/time> |
| time-macros | 0.2.32 | MIT OR Apache-2.0 | <https://github.com/time-rs/time> |
| tiny_http | 0.12.0 | MIT OR Apache-2.0 | <https://github.com/tiny-http/tiny-http> |
| tinystr | 0.8.5 | Unicode-3.0 | <https://github.com/unicode-org/icu4x> |
| toml | 1.2.1+spec-1.1.0 | MIT OR Apache-2.0 | <https://github.com/toml-rs/toml> |
| toml_datetime | 1.1.3+spec-1.1.0 | MIT OR Apache-2.0 | <https://github.com/toml-rs/toml> |
| toml_parser | 1.1.5+spec-1.1.0 | MIT OR Apache-2.0 | <https://github.com/toml-rs/toml> |
| toml_writer | 1.2.0+spec-1.1.0 | MIT OR Apache-2.0 | <https://github.com/toml-rs/toml> |
| unicode-ident | 1.0.27 | (MIT OR Apache-2.0) AND Unicode-3.0 | <https://github.com/dtolnay/unicode-ident> |
| unicode-segmentation | 1.13.3 | MIT OR Apache-2.0 | <https://github.com/unicode-rs/unicode-segmentation> |
| url | 2.5.8 | MIT OR Apache-2.0 | <https://github.com/servo/rust-url> |
| utf8_iter | 1.1.0 | Apache-2.0 OR MIT | <https://github.com/hsivonen/utf8_iter> |
| utf_types | 1.0.0 | Apache-2.0 OR MIT | <https://github.com/hsivonen/utf_types> |
| version_check | 0.9.5 | MIT/Apache-2.0 | <https://github.com/SergioBenitez/version_check> |
| webview2-com | 0.39.1 | MIT | <https://github.com/wravery/webview2-rs> |
| webview2-com-macros | 0.8.1 | MIT | <https://github.com/wravery/webview2-rs> |
| webview2-com-sys | 0.39.1 | MIT | <https://github.com/wravery/webview2-rs> |
| windows | 0.62.2 | MIT OR Apache-2.0 | <https://github.com/microsoft/windows-rs> |
| windows-collections | 0.3.2 | MIT OR Apache-2.0 | <https://github.com/microsoft/windows-rs> |
| windows-core | 0.62.2 | MIT OR Apache-2.0 | <https://github.com/microsoft/windows-rs> |
| windows-future | 0.3.2 | MIT OR Apache-2.0 | <https://github.com/microsoft/windows-rs> |
| windows-implement | 0.60.2 | MIT OR Apache-2.0 | <https://github.com/microsoft/windows-rs> |
| windows-interface | 0.59.3 | MIT OR Apache-2.0 | <https://github.com/microsoft/windows-rs> |
| windows-link | 0.2.1 | MIT OR Apache-2.0 | <https://github.com/microsoft/windows-rs> |
| windows-numerics | 0.3.1 | MIT OR Apache-2.0 | <https://github.com/microsoft/windows-rs> |
| windows-result | 0.4.1 | MIT OR Apache-2.0 | <https://github.com/microsoft/windows-rs> |
| windows-strings | 0.5.1 | MIT OR Apache-2.0 | <https://github.com/microsoft/windows-rs> |
| windows-sys | 0.61.2 | MIT OR Apache-2.0 | <https://github.com/microsoft/windows-rs> |
| windows-threading | 0.2.1 | MIT OR Apache-2.0 | <https://github.com/microsoft/windows-rs> |
| windows-version | 0.1.7 | MIT OR Apache-2.0 | <https://github.com/microsoft/windows-rs> |
| winnow | 1.0.5 | MIT | <https://github.com/winnow-rs/winnow> |
| writeable | 0.6.4 | Unicode-3.0 | <https://github.com/unicode-org/icu4x> |
| wry | 0.57.2 | Apache-2.0 OR MIT | <https://github.com/tauri-apps/wry> |
| yoke | 0.8.4 | Unicode-3.0 | <https://github.com/unicode-org/icu4x> |
| yoke-derive | 0.8.4 | Unicode-3.0 | <https://github.com/unicode-org/icu4x> |
| zerofrom | 0.1.8 | Unicode-3.0 | <https://github.com/unicode-org/icu4x> |
| zerofrom-derive | 0.1.8 | Unicode-3.0 | <https://github.com/unicode-org/icu4x> |
| zerotrie | 0.2.5 | Unicode-3.0 | <https://github.com/unicode-org/icu4x> |
| zerovec | 0.11.9 | Unicode-3.0 | <https://github.com/unicode-org/icu4x> |
| zerovec-derive | 0.11.7 | Unicode-3.0 | <https://github.com/unicode-org/icu4x> |
| zmij | 1.0.23 | MIT | <https://github.com/dtolnay/zmij> |
