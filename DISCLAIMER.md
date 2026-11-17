# Disclaimer

This notice covers pingtable: its source code, the programs built from it, the
files in each release, and the pingtable.net website. It sets out in plain
words what already applies under the
[Apache License, Version 2.0](LICENSE). It adds no conditions to that license
and takes no right away from you. If this notice and the license ever differ,
the license governs.

## No warranty

pingtable is provided "AS IS" and "AS AVAILABLE", WITHOUT WARRANTIES OR
CONDITIONS OF ANY KIND, express, implied or statutory. That includes, without
limitation, any warranty or condition of title, non-infringement,
merchantability, fitness for a particular purpose, accuracy, reliability or
availability. No statement made by the authors, in any form, creates a
warranty.

In particular, nobody promises that:

- a measurement, status, alert, report or export is correct, complete or on
  time;
- an outage will be noticed, or that a reported outage is real;
- the software is free of defects, will run without interruption, or is safe
  for your computers and networks;
- a downloaded file is free of harmful code. The release files are not
  code-signed. Compare the published SHA-256, or build from source.

## No liability

TO THE MAXIMUM EXTENT PERMITTED BY APPLICABLE LAW, IN NO EVENT AND UNDER NO
LEGAL THEORY, whether in tort (including negligence), contract or otherwise,
SHALL LORINCZ LLC, THE AUTHORS OR ANY CONTRIBUTOR BE LIABLE TO YOU FOR ANY
DAMAGES, including any direct, indirect, special, incidental, consequential,
exemplary or punitive damages of any character, arising out of the license or
out of the use of, or the inability to use, pingtable. That includes, without
limitation, damages for loss of data, loss of profits or goodwill, work
stoppage, network or service disruption, missed or false alerts, computer
failure or malfunction, security incidents, regulatory or contractual
penalties, and claims made by third parties, even if the possibility of such
damages was known.

Some places do not allow certain warranties to be excluded or certain
liability to be limited. There, the exclusions above apply to the fullest
extent the law allows.

## You use it at your own risk

You alone decide whether pingtable suits your purpose, and you carry every
risk of using it.

- **Permission.** pingtable sends network traffic to every host in your list:
  ICMP echo requests, TCP connection attempts, route traces and name lookups.
  A range or an address block can expand to many hosts, and a measurement can
  send up to ten probes a second to each one. Use it only on networks and
  hosts you own or have clear permission to test. Probing without permission
  can breach contracts, acceptable-use policies and the law.
- **Rules that apply to you.** You are responsible for complying with the
  laws, regulations, employer and network policies and third-party terms that
  apply to you, including export and sanctions rules.
- **Commands.** pingtable can run commands that you set up for alerts. What
  those commands do is your responsibility.
- **Your data.** Keep your own copies of anything you need. pingtable stores
  its settings, host list and mutes, and may lose or overwrite them.
- **Not for critical use.** pingtable is not designed, tested or intended for
  any use where a failure or a wrong reading could lead to death, injury, or
  serious physical, financial or environmental harm. It does not replace a
  professionally run monitoring service. Do not make it your only safeguard.

## No support and no promise that it continues

Nobody is obliged to provide support, maintenance, fixes, updates or security
patches, or to keep any feature, file format, API, download or website
available. pingtable may change or stop at any time without notice.

## Other people's work and names

pingtable includes open-source components and, in the Windows package,
Microsoft's WebView2 loader. Their own authors provide them under their own
terms, listed in [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md), with no
warranty from anyone involved in pingtable.

pingtable is an independent project. It is not affiliated with, endorsed by or
sponsored by Microsoft or by the maker of any other network tool. Product and
company names appear only to identify those products, for example to say which
file formats can be imported, and are the trademarks of their owners.

The name "pingtable" and its mark are not licensed for use as your own name or
brand (section 6 of the license).

## Not advice

Nothing in this repository is legal, security or compliance advice.

## Contact

pingtable@lorincz.llc
