# {{ repo }} Release Kickoff

- Base tag: {{ base_tag }}
- Main crate: {{ main_crate }}
- Proposed release date: {{ release_date }}

Workspace crates:
{% for crate in crates %}- {{ crate.name }} {{ crate.version }}
{% endfor %}

Please add agenda items, blockers, and verification tasks below. Once scope is agreed, run `asfship prerelease` to prepare the first release candidate.
