// Synthetic trace replay for the Agent Development Workbench.
// D3 handles only presentation; validation, redaction and correlation happen
// in Rust before this endpoint can return an event.
(() => {
	const state = {
		data: null,
		fixture: null,
		selectedEventId: null,
		mode: 'debug'
	};

	const elements = {};

	const titleCase = (value) =>
		value
			.split('_')
			.map((part) => part.charAt(0).toUpperCase() + part.slice(1))
			.join(' ');

	const selectedEvent = () => state.fixture?.trace.events.find((event) => event.event_id === state.selectedEventId);

	const selectedExplanation = () =>
		state.fixture?.explanations.find((explanation) => explanation.grounded_in_event_id === state.selectedEventId);

	function selectEvent(eventId) {
		state.selectedEventId = eventId;
		renderTimeline();
		renderDetails();
	}

	function chooseDefaultEvent() {
		const failed = state.fixture.trace.events.find((event) => event.status === 'failed');
		state.selectedEventId = (failed ?? state.fixture.trace.events[0]).event_id;
	}

	function renderSummary() {
		const events = state.fixture.trace.events;
		const terminal = events.at(-1);
		const totalDuration = state.fixture.timeline.reduce(
			(maximum, row) => Math.max(maximum, row.offset_ms + row.duration_ms),
			0
		);
		const failed = events.filter((event) => event.status === 'failed').length;
		const redactions = events.reduce((sum, event) => sum + event.redacted_fields.length, 0);

		elements.summary.innerHTML = '';
		for (const [label, value, className] of [
			['Status', titleCase(terminal.status), `pill-${terminal.status === 'failed' ? 'error' : 'success'}`],
			['Events', String(events.length), ''],
			['Duration', `${totalDuration} ms`, ''],
			['Failures', String(failed), failed ? 'pill-error' : '']
		]) {
			const item = document.createElement('span');
			item.className = `pill ${className}`.trim();
			item.textContent = `${label}: ${value}`;
			elements.summary.append(item);
		}

		elements.schema.textContent = state.data.event_schema_version;
		elements.source.textContent = state.data.data_source;
		elements.redactions.textContent = String(redactions);
	}

	function renderTimeline() {
		if (!window.d3 || !state.fixture) return;

		const rows = state.fixture.timeline;
		const containerWidth = Math.max(elements.chart.clientWidth, 720);
		const margin = { top: 42, right: 30, bottom: 24, left: 112 };
		const components = ['harness', 'model', 'mcp', 'retrieval', 'memory', 'application'].filter((component) =>
			rows.some((row) => row.component === component)
		);
		const laneHeight = 56;
		const width = containerWidth;
		const height = margin.top + margin.bottom + components.length * laneHeight;
		const innerWidth = width - margin.left - margin.right;
		const maximum = d3.max(rows, (row) => row.offset_ms + row.duration_ms) || 1;

		const x = d3.scaleLinear().domain([0, maximum]).nice().range([0, innerWidth]);
		const y = d3.scaleBand().domain(components).range([0, components.length * laneHeight]).padding(0.25);
		const svg = d3.select(elements.timeline).attr('viewBox', `0 0 ${width} ${height}`).attr('height', height);
		svg.selectAll('*').remove();

		const chart = svg.append('g').attr('transform', `translate(${margin.left},${margin.top})`);
		chart
			.append('g')
			.attr('class', 'workbench-axis')
			.call(d3.axisTop(x).ticks(Math.max(3, Math.floor(innerWidth / 140))).tickFormat((value) => `${value} ms`));

		chart
			.selectAll('.workbench-lane')
			.data(components)
			.join('rect')
			.attr('class', 'workbench-lane')
			.attr('x', 0)
			.attr('y', (component) => y(component))
			.attr('width', innerWidth)
			.attr('height', y.bandwidth());

		chart
			.selectAll('.workbench-lane-label')
			.data(components)
			.join('text')
			.attr('class', 'workbench-lane-label')
			.attr('x', -12)
			.attr('y', (component) => y(component) + y.bandwidth() / 2)
			.attr('text-anchor', 'end')
			.attr('dominant-baseline', 'middle')
			.text(titleCase);

		const byOperation = new Map(rows.map((row) => [row.operation_id, row]));
		const selectedLineage = new Set();
		let lineageEvent = rows.find((row) => row.event_id === state.selectedEventId);
		while (lineageEvent) {
			selectedLineage.add(lineageEvent.operation_id);
			lineageEvent = lineageEvent.parent_operation_id
				? byOperation.get(lineageEvent.parent_operation_id)
				: null;
		}
		const links = rows
			.filter((row) => row.parent_operation_id && byOperation.has(row.parent_operation_id))
			.map((row) => ({ parent: byOperation.get(row.parent_operation_id), child: row }));

		chart
			.append('g')
			.attr('class', 'workbench-links')
			.selectAll('path')
			.data(links)
			.join('path')
			.classed(
				'selected',
				({ parent, child }) =>
					selectedLineage.has(parent.operation_id) && selectedLineage.has(child.operation_id)
			)
			.attr('d', ({ parent, child }) => {
				const parentX = x(parent.offset_ms);
				const parentY = y(parent.component) + y.bandwidth() / 2;
				const childX = x(child.offset_ms);
				const childY = y(child.component) + y.bandwidth() / 2;
				const midpoint = parentX + (childX - parentX) / 2;
				return `M${parentX},${parentY} C${midpoint},${parentY} ${midpoint},${childY} ${childX},${childY}`;
			});

		const eventGroups = chart
			.append('g')
			.selectAll('.workbench-event')
			.data(rows, (row) => row.event_id)
			.join('g')
			.attr(
				'class',
				(row) =>
					`workbench-event event-${row.status}${row.event_id === state.selectedEventId ? ' selected' : ''}`
			)
			.attr('tabindex', 0)
			.attr('role', 'button')
			.attr('aria-label', (row) => `${titleCase(row.kind)}, ${titleCase(row.status)}, ${row.offset_ms} milliseconds`)
			.on('click', (_, row) => selectEvent(row.event_id))
			.on('keydown', (event, row) => {
				if (event.key === 'Enter' || event.key === ' ') {
					event.preventDefault();
					selectEvent(row.event_id);
				}
			});

		eventGroups
			.append('rect')
			.attr('x', (row) => x(row.offset_ms))
			.attr('y', (row) => y(row.component))
			.attr('width', (row) => Math.max(7, x(row.offset_ms + row.duration_ms) - x(row.offset_ms)))
			.attr('height', y.bandwidth())
			.attr('rx', 5);

		eventGroups
			.append('title')
			.text((row) => `${titleCase(row.kind)}\n${row.offset_ms} ms · ${row.duration_ms} ms\nDepth ${row.depth}`);
	}

	function renderDetails() {
		const event = selectedEvent();
		const explanation = selectedExplanation();
		if (!event || !explanation) return;

		elements.eventJson.textContent = JSON.stringify(event, null, 2);
		elements.learnSummary.textContent = explanation.summary;
		elements.learnSignificance.textContent = explanation.significance;
		elements.learnEvidence.textContent = explanation.evidence_note;
		elements.debug.hidden = state.mode !== 'debug';
		elements.learn.hidden = state.mode !== 'learn';
	}

	function chooseFixture(id) {
		state.fixture = state.data.fixtures.find((fixture) => fixture.id === id) ?? state.data.fixtures[0];
		chooseDefaultEvent();
		renderSummary();
		renderTimeline();
		renderDetails();
	}

	async function initialize() {
		Object.assign(elements, {
			fixture: document.getElementById('workbench-fixture'),
			summary: document.getElementById('workbench-run-summary'),
			chart: document.getElementById('workbench-chart'),
			timeline: document.getElementById('workbench-timeline'),
			debug: document.getElementById('workbench-debug'),
			learn: document.getElementById('workbench-learn'),
			eventJson: document.getElementById('workbench-event-json'),
			learnSummary: document.getElementById('workbench-learn-summary'),
			learnSignificance: document.getElementById('workbench-learn-significance'),
			learnEvidence: document.getElementById('workbench-learn-evidence'),
			schema: document.getElementById('workbench-schema'),
			source: document.getElementById('workbench-source'),
			redactions: document.getElementById('workbench-redactions'),
			error: document.getElementById('workbench-error')
		});
		if (!elements.timeline) return;

		try {
			if (!window.d3) throw new Error('D3 failed to load.');
			const response = await fetch('/workbench/data', { headers: { Accept: 'application/json' }, cache: 'no-store' });
			if (!response.ok) throw new Error(`Trace endpoint returned HTTP ${response.status}.`);
			state.data = await response.json();
			if (!state.data.fixtures?.length) throw new Error('No trace fixtures were returned.');

			elements.fixture.innerHTML = '';
			for (const fixture of state.data.fixtures) {
				const option = document.createElement('option');
				option.value = fixture.id;
				option.textContent = fixture.label;
				elements.fixture.append(option);
			}
			elements.fixture.disabled = false;
			elements.fixture.addEventListener('change', () => chooseFixture(elements.fixture.value));
			document.querySelectorAll('input[name="workbench-mode"]').forEach((input) => {
				input.addEventListener('change', () => {
					state.mode = input.value;
					renderDetails();
				});
			});

			chooseFixture(state.data.fixtures[0].id);
			new ResizeObserver(() => renderTimeline()).observe(elements.chart);
		} catch (error) {
			elements.error.textContent = error instanceof Error ? error.message : String(error);
		}
	}

	window.addEventListener('DOMContentLoaded', initialize);
})();
