# [VOTE] {{ repo }} {{ version }}{{ rc_suffix }}

Artifacts are available at:
- SVN: {{ svn_url }}

Artifacts and checksums:
{% for a in artifacts %}- {{ a.name }}{% if a.sha512 %} (sha512={{ a.sha512 }}){% endif %} — {{ a.url }}
{% endfor %}

Please vote within the specified period. Proposed close date: {{ vote_close_date }}.
