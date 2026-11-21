# Warranty and liability

Lorincz LLC distributes the original pingtable software under the Apache License, Version 2.0. Sections 7 through 9 govern warranty, liability and additional commitments. Applicable law and any separate written agreement govern where those sections provide exceptions. This summary adds no license conditions. Third-party components retain their own terms. [Apache License 2.0](LICENSE) · [Third-party notices](THIRD-PARTY-NOTICES.md)

## Measurements and outages

The software is provided as is, subject to those exceptions. The project gives no assurance of accuracy, availability, timely alerts or fitness for a particular purpose. Firewalls can discard a probe while a service is healthy. A successful probe can coincide with an application outage. Confirm important results independently and provide separate safeguards for safety-critical or business-critical decisions.

## Operational and security risks

- Probes, name lookups and repeated connection attempts generate traffic. Address ranges can expand into many destinations. Choose a rate and scope appropriate to the network and obtain authorization to test those systems.
- Alert commands execute locally with the operating-system permissions of the account running pingtable. Configuration changes, imported labels and a compromised account or API token can affect the commands or destinations involved. Run with ordinary user privileges and restrict access to your settings. [Local controls and private reporting](SECURITY.md)
- Hosts, addresses, timestamps and other network information may appear in configurations, logs, exports and the clipboard. Protect these records, review them before sharing and keep recoverable copies of important settings and data.

To the extent provided by section 8, the limitation of liability applies to damages arising from use or inability to use pingtable. Examples include loss or corruption of data, missed alerts, network disruption, downtime, unauthorized access or compromise, and resulting business losses. Rights and remedies that applicable law preserves remain available.

Software downloads and a support address carry no service-level commitment. The status of an individual build's signature and its published digest are documented with that release. Security reports go to [support@pingtable.net](mailto:support@pingtable.net).
