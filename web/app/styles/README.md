# Workspace theme

`theme.scss` is the application stylesheet entry point. `tokens` defines the
4 px spacing scale and control dimensions. `base` and `workspace` own structural
layout; `controls` owns button and input appearance across navigation, inspectors
and dialogs. Components retain domain-specific geometry and layout.

Buttons have one 44 px minimum height, pill radius, typography and state model.
Use the ordinary button for secondary actions, `primary` for the committing or
main action, and `danger` for destructive intent. `aria-pressed` and the active
navigation state identify selection. Resource names and sort headings are inline
actions; close/add icons have a square target using the same control height.
Labels may wrap rather than truncate an action's meaning.

The theme supplies hover, pressed, focus-visible and disabled states. Disabled
controls stay legible. Do not encode unknown inventory knowledge by fading
interactive controls. Shared appearance belongs here rather than in a page's
scoped CSS. Responsive changes may rearrange actions without changing their
meaning, and retain the accessible label and keyboard path.

`scene` owns the physical canvas, inspector and responsive selection layout.
Phone lists reserve no checkbox column until Select is enabled. Tablet lists
show selection controls; fine-pointer desktop lists reveal them on hover,
keyboard focus or an active selection. Hidden controls must not prevent keyboard
access on desktop or leave an empty column on a phone.
