// Chat: sends messages over the existing WebSocket, shows LLM rounds and tool
// calls live, then replaces the live view with the server-rendered message.

(() => {
	const scroll = document.querySelector('[data-chat]');
	if (!scroll) return;
	const conversationId = scroll.dataset.conversationId;
	const list = document.getElementById('messages');
	const form = document.getElementById('chat-form');
	const input = document.getElementById('chat-input');
	const provider = document.getElementById('chat-provider');
	const sendButton = document.getElementById('chat-send');
	const status = document.getElementById('chat-status');
	const errorBox = document.getElementById('chat-error');

	let ws = null;
	let sending = false;
	let live = null; // { item, timeline }
	let retry = 0;

	const scrollDown = () => requestAnimationFrame(() => (scroll.scrollTop = scroll.scrollHeight));
	const el = (tag, className, text) => {
		const node = document.createElement(tag);
		if (className) node.className = className;
		if (text != null) node.textContent = text;
		return node;
	};
	const setSending = (value) => {
		sending = value;
		sendButton.disabled = value;
		status.textContent = value ? 'Waiting for the answer…' : '';
	};

	function liveView() {
		if (live) return live;
		const item = el('li', 'msg msg-assistant');
		const trace = el('div', 'msg-trace');
		const timeline = el('ol', 'timeline');
		timeline.setAttribute('aria-label', 'How this answer is being made');
		trace.append(timeline);
		const bubble = el('div', 'bubble');
		bubble.append(el('p', 'faint', 'Working…'));
		item.append(trace, bubble);
		list.append(item);
		document.querySelector('.chat-empty')?.remove();
		live = { item, timeline };
		return live;
	}

	function row(kind, summary, meta, extraClass) {
		const li = el('li');
		const details = el('details', `step ${extraClass || ''}`);
		const sum = el('summary');
		sum.append(el('span', 'step-kind', kind), el('span', 'step-summary', summary));
		if (meta) sum.append(el('span', 'step-meta nums', meta));
		details.append(sum);
		li.append(details);
		return { li, details, sum };
	}

	function addStep(step) {
		const { timeline } = liveView();
		const calls = step.response?.tool_calls?.map((c) => c.name) ?? [];
		const summary = step.error ? 'failed' : calls.length ? `asked for ${calls.join(', ')}` : 'answered';
		const ms = step.duration_ms < 1000 ? `${step.duration_ms} ms` : `${(step.duration_ms / 1000).toFixed(1)} s`;
		const tokens = step.usage?.prompt_tokens != null ? ` · ${step.usage.prompt_tokens} → ${step.usage.completion_tokens ?? 0} tokens` : '';
		const r = row(`LLM · round ${step.round}`, summary, ms + tokens, step.error ? 'failed' : '');
		const body = el('div', 'step-body');
		body.append(el('pre', 'code', JSON.stringify(step.error ? { error: step.error } : step.response, null, 2)));
		r.details.append(body);
		timeline.append(r.li);
		const text = step.response?.content;
		if (text && calls.length) r.li.append(el('p', 'step-text pre-wrap', text));
		scrollDown();
	}

	// MCP tool arguments arrive as a JSON string or as an object
	const pretty = (value) => {
		if (typeof value === 'string') {
			try {
				return JSON.stringify(JSON.parse(value), null, 2);
			} catch {
				return value;
			}
		}
		return JSON.stringify(value ?? {}, null, 2);
	};

	function addToolCall(name, args) {
		const { timeline } = liveView();
		const r = row('Tool', name, null, 'tool');
		r.sum.querySelector('.step-summary').classList.add('mono');
		const spinner = el('span', 'spinner');
		spinner.setAttribute('aria-label', 'running');
		r.sum.append(spinner);
		const body = el('div', 'step-body');
		body.append(el('p', 'faint tiny', 'Arguments'), el('pre', 'code', pretty(args)));
		r.details.append(body);
		r.li.dataset.tool = name;
		timeline.append(r.li);
		scrollDown();
	}

	// Same parts as the saved view (macros.html tool_result): text items, images, raw JSON
	function toolResult(result) {
		const failed = result?.isError === true;
		const box = el('div', `tool-result${failed ? ' failed' : ''}`);
		if (failed) {
			const p = el('p', 'error-text');
			p.append(el('strong', null, 'Tool returned an error'));
			box.append(p);
		}
		const items = Array.isArray(result?.content) ? result.content : [];
		let shown = 0;
		for (const item of items) {
			if (item?.type === 'text' && typeof item.text === 'string') {
				box.append(el('p', 'pre-wrap', item.text));
				shown++;
			} else if (item?.type === 'image' && typeof item.data === 'string' && /^image\/[\w.+-]+$/.test(item.mimeType ?? '')) {
				const img = el('img');
				img.src = `data:${item.mimeType};base64,${item.data}`;
				img.alt = 'Tool output';
				box.append(img);
				shown++;
			}
		}
		const raw = el('details');
		if (!shown) raw.open = true;
		raw.append(el('summary', 'faint tiny', 'Raw result'), el('pre', 'code', JSON.stringify(result, null, 2)));
		box.append(raw);
		return box;
	}

	function toolDone(name, result) {
		const pending = [...live?.timeline.querySelectorAll('li[data-tool]') ?? []].find(
			(li) => li.dataset.tool === name && li.querySelector('.spinner')
		);
		if (!pending) return;
		const failed = result?.isError === true;
		pending.querySelector('.spinner').replaceWith(el('span', `pill ${failed ? 'pill-error' : 'pill-success'}`, failed ? 'error' : 'done'));
		pending.querySelector('.step-body')?.append(el('p', 'faint tiny', 'Result'), toolResult(result));
	}

	async function complete(messageId) {
		try {
			const response = await fetch(`/chat/${conversationId}/messages/${messageId}`, { headers: { 'HX-Request': 'true' } });
			if (!response.ok) throw new Error(`HTTP ${response.status}`);
			const html = await response.text();
			const template = document.createElement('template');
			template.innerHTML = html.trim();
			const node = template.content.firstElementChild;
			if (live && node) live.item.replaceWith(node);
			else if (node) list.append(node);
			if (node) window.localizeTimes?.(node);
		} catch {
			// Fall back to a reload so the saved answer appears
			location.reload();
		}
		live = null;
		setSending(false);
		scrollDown();
		input.focus();
	}

	function handle(data) {
		switch (data.type) {
			case 'user_message': {
				const item = el('li', 'msg msg-user');
				const bubble = el('div', 'bubble');
				bubble.append(el('p', 'pre-wrap', data.content), el('time', 'faint tiny', new Date().toTimeString().slice(0, 5)));
				item.append(bubble);
				list.append(item);
				document.querySelector('.chat-empty')?.remove();
				liveView();
				scrollDown();
				break;
			}
			case 'llm_step':
				addStep(data.step);
				break;
			case 'tool_call':
				addToolCall(data.tool_name, data.arguments);
				break;
			case 'tool_result':
				toolDone(data.tool_name, data.result);
				break;
			case 'assistant_complete':
				complete(data.message_id);
				break;
			case 'error':
				errorBox.textContent = data.message;
				if (live) live.item.querySelector('.bubble').replaceChildren(el('p', 'error-text', 'No answer.'));
				live = null;
				setSending(false);
				break;
		}
	}

	function connect() {
		const scheme = location.protocol === 'https:' ? 'wss' : 'ws';
		ws = new WebSocket(`${scheme}://${location.host}/api/v1/ws/conversations/${conversationId}`);
		ws.addEventListener('open', () => {
			retry = 0;
			status.textContent = sending ? status.textContent : '';
		});
		ws.addEventListener('message', (event) => {
			try {
				handle(JSON.parse(event.data));
			} catch (e) {
				console.error('Bad chat event', e);
			}
		});
		ws.addEventListener('close', () => {
			if (sending) {
				errorBox.textContent = 'The connection closed before the answer arrived.';
				setSending(false);
			}
			retry = Math.min(retry + 1, 5);
			status.textContent = 'Reconnecting…';
			setTimeout(connect, 500 * 2 ** retry);
		});
	}

	function send() {
		const content = input.value.trim();
		if (!content || sending || ws?.readyState !== WebSocket.OPEN) return;
		errorBox.textContent = '';
		ws.send(JSON.stringify({ content, provider_id: provider?.value ? Number(provider.value) : null }));
		input.value = '';
		setSending(true);
	}

	form.addEventListener('submit', (event) => {
		event.preventDefault();
		send();
	});
	// Enter sends, Shift+Enter makes a new line; never while an IME is composing
	input.addEventListener('keydown', (event) => {
		if (event.key === 'Enter' && !event.shiftKey && !event.isComposing) {
			event.preventDefault();
			send();
		}
	});

	// MCP servers on or off for this conversation; the next message uses the new choice
	const serversCount = document.querySelector('[data-servers-count]');
	const serverBoxes = [...document.querySelectorAll('input[data-server-id]')];
	const countServers = () => {
		if (serversCount) serversCount.textContent = `${serverBoxes.filter((b) => b.checked).length}/${serverBoxes.length}`;
	};
	serverBoxes.forEach((box) =>
		box.addEventListener('change', async () => {
			countServers();
			box.disabled = true;
			try {
				const response = await fetch(`/api/v1/conversations/${conversationId}/servers/${box.dataset.serverId}`, {
					method: 'PUT',
					headers: { 'Content-Type': 'application/json' },
					body: JSON.stringify({ enabled: box.checked }),
				});
				if (!response.ok) throw new Error(`HTTP ${response.status}`);
				errorBox.textContent = '';
			} catch {
				box.checked = !box.checked;
				countServers();
				errorBox.textContent = 'Could not change the MCP server for this conversation.';
			} finally {
				box.disabled = false;
			}
		})
	);

	connect();
	scrollDown();
})();
