# {{ repo }} {{ version }} Released

Stable tag: {{ tag }} (promoted from {{ rc_tag }})

Changed crates:
{% for c in crates %}- {{ c.name }}: {{ c.old_version }} → {{ c.new_version }}
{% endfor %}
