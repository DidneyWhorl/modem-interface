'use strict';
'require view';

return view.extend({
	handleSaveApply: null,
	handleSave: null,
	handleReset: null,

	render: function() {
		var proto = (window.location.protocol === 'https:') ? 'https' : 'http';
		var port = (proto === 'https') ? '8443' : '8082';
		var target = proto + '://' + window.location.hostname + ':' + port + '/ctrl-modem/home';

		window.location.href = target;

		return E('div', { 'class': 'cbi-map' },
			E('p', {}, 'Redirecting to CTRL-Modem...')
		);
	}
});
