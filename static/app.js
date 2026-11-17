// Small helpers for the server-rendered UI. No framework, no build step.

// htmx: swap 422 responses (validation fragments) like successes; other
// 4xx/5xx are errors that leave the page as it is.
document.addEventListener('DOMContentLoaded', () => {
	if (window.htmx) {
		htmx.config.responseHandling = [
			{ code: '204', swap: false },
			{ code: '[23]..', swap: true },
			{ code: '422', swap: true },
			{ code: '[45]..', swap: false, error: true }
		];
	}
});

// Dialogs: <button data-open-dialog="id"> opens a native modal dialog,
// [data-close-dialog] inside a dialog closes it.
document.addEventListener('click', (event) => {
	const opener = event.target.closest('[data-open-dialog]');
	if (opener) {
		const dialog = document.getElementById(opener.dataset.openDialog);
		if (dialog) {
			if (opener.hasAttribute('data-reset')) dialog.querySelector('form')?.reset();
			dialog.querySelectorAll('[data-clear-on-open]').forEach((el) => (el.innerHTML = ''));
			dialog.showModal();
		}
		return;
	}
	const closer = event.target.closest('[data-close-dialog]');
	if (closer) closer.closest('dialog')?.close();
});

// Light dismiss for dialog[closedby="any"] where the attribute is unsupported:
// close only when press and release both happen outside the dialog box.
if (!('closedBy' in HTMLDialogElement.prototype)) {
	const outside = (e, d) => {
		const r = d.getBoundingClientRect();
		return e.clientX < r.left || e.clientX > r.right || e.clientY < r.top || e.clientY > r.bottom;
	};
	let pressed = null;
	document.addEventListener('pointerdown', (e) => {
		const d = e.target.closest?.('dialog[closedby="any"]');
		pressed = d && outside(e, d) ? d : null;
	});
	document.addEventListener('pointerup', (e) => {
		if (pressed && e.target.closest?.('dialog') === pressed && outside(e, pressed)) pressed.close();
		pressed = null;
	});
}

// Close the surrounding dialog after a successful htmx request from a
// form marked data-close-on-success.
// Only the form's own submit counts, not other htmx buttons inside it
// (e.g. "Load models" or "Test").
document.addEventListener('htmx:afterRequest', (event) => {
	const form = event.target;
	if (!form.matches?.('form[data-close-on-success]')) return;
	if (event.detail.successful && event.detail.xhr.status < 300) {
		form.closest('dialog')?.close();
	}
});

// hx-confirm with a themed native dialog instead of window.confirm().
document.addEventListener('htmx:confirm', (event) => {
	if (!event.detail.question) return;
	const dialog = document.getElementById('confirm-dialog');
	if (!dialog) return;
	event.preventDefault();
	dialog.querySelector('[data-confirm-message]').textContent = event.detail.question;
	dialog.returnValue = '';
	dialog.addEventListener(
		'close',
		() => {
			if (dialog.returnValue === 'confirm') event.detail.issueRequest(true);
			dialog.returnValue = '';
		},
		{ once: true }
	);
	dialog.showModal();
});

// Fill form fields from the selected <option data-*> of a select[data-preset].
// data-name / data-base-url / data-model go to the inputs named in
// data-preset-fields ("name:#id,base-url:#id,model:#id").
document.addEventListener('change', (event) => {
	const select = event.target.closest?.('select[data-preset]');
	if (!select) return;
	const option = select.selectedOptions[0];
	if (!option || !option.dataset.baseUrl) return;
	for (const pair of select.dataset.preset.split(',')) {
		const [key, target] = pair.split(':');
		const input = document.querySelector(target);
		const value = option.dataset[key.replace(/-(.)/g, (_, c) => c.toUpperCase())];
		if (input && value != null) input.value = value;
	}
});

// User menu popover: position from script where CSS anchor positioning is
// missing; open/close by hand where the Popover API is missing.
(() => {
	const menu = document.getElementById('user-menu');
	const trigger = document.querySelector('[popovertarget="user-menu"]');
	if (!menu || !trigger) return;
	const place = () => {
		if (CSS.supports('position-area', 'block-end')) return;
		const r = trigger.getBoundingClientRect();
		menu.style.top = `${r.bottom + 6}px`;
		menu.style.left = `${Math.max(8, Math.min(r.right - menu.offsetWidth, innerWidth - menu.offsetWidth - 8))}px`;
	};
	if (Object.hasOwn(HTMLElement.prototype, 'popover')) {
		menu.addEventListener('toggle', (e) => {
			trigger.setAttribute('aria-expanded', String(e.newState === 'open'));
			if (e.newState === 'open') place();
		});
	} else {
		menu.hidden = true;
		trigger.addEventListener('click', () => {
			menu.hidden = !menu.hidden;
			trigger.setAttribute('aria-expanded', String(!menu.hidden));
			if (!menu.hidden) place();
		});
		document.addEventListener('keydown', (e) => {
			if (e.key === 'Escape' && !menu.hidden) {
				menu.hidden = true;
				trigger.focus();
			}
		});
	}
})();

// A select[data-other] with an "Other" option (value "__other__") turns into
// a text input with the same id and name, so any value can be typed.
document.addEventListener('change', (event) => {
	const select = event.target.closest?.('select[data-other]');
	if (!select || select.value !== '__other__') return;
	const input = document.createElement('input');
	input.className = 'input';
	input.id = select.id;
	input.name = select.name;
	input.required = select.required;
	input.autocomplete = 'off';
	input.placeholder = 'Model name';
	for (const attr of ['aria-describedby']) {
		if (select.hasAttribute(attr)) input.setAttribute(attr, select.getAttribute(attr));
	}
	select.replaceWith(input);
	input.focus();
});

// Server responses can close the open dialog with "HX-Trigger: close-dialog".
document.addEventListener('close-dialog', () => {
	document.querySelectorAll('dialog[open]').forEach((d) => d.close());
});

// <input data-filter-list="#list"> hides list items whose text doesn't match.
document.addEventListener('input', (event) => {
	const input = event.target.closest?.('[data-filter-list]');
	if (!input) return;
	const query = input.value.trim().toLowerCase();
	document.querySelectorAll(`${input.dataset.filterList} > li`).forEach((li) => {
		li.hidden = query !== '' && !li.textContent.toLowerCase().includes(query);
	});
});

// <time datetime=... data-local-time> shows the time in the viewer's time zone
window.localizeTimes = (root = document) => {
	root.querySelectorAll('time[data-local-time]').forEach((t) => {
		const date = new Date(t.dateTime);
		if (!Number.isNaN(date.getTime())) {
			t.textContent = date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
		}
	});
};
document.addEventListener('DOMContentLoaded', () => window.localizeTimes());
document.addEventListener('htmx:afterSwap', (e) => window.localizeTimes(e.target));
