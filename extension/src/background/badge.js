export class Badge {
  constructor(chrome) {
    this.chrome = chrome;
    this.count = 0;
    chrome.alarms.onAlarm.addListener(alarm => { if (alarm.name === 'siphon:badge') void this.render(); });
  }
  async setCount(count) { this.count = count; await this.render(); }
  async render() {
    await this.chrome.action.setBadgeBackgroundColor({ color: '#6052d6' });
    await this.chrome.action.setBadgeText({ text: this.count ? String(this.count) : '' });
  }
  async flash(success, text = success ? '✓' : '!') {
    if (this.count) return this.render();
    await this.chrome.action.setBadgeBackgroundColor({ color: success ? '#178558' : '#c13743' });
    await this.chrome.action.setBadgeText({ text });
    await this.chrome.alarms.create('siphon:badge', { when: Date.now() + 4000 });
  }
}
