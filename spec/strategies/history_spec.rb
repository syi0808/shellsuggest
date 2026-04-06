# Tests basic history prefix matching behavior
# Uses real command execution to populate the daemon journal

describe 'history suggestion' do
  it 'suggests the last matching history entry' do
    session.run_command('echo foo')
    session.run_command('echo bar')
    session.run_command('echo baz')
    sleep 0.3
    session.clear_screen

    session.send_string('echo b')
    wait_for { session.content }.to eq('echo baz')
  end

  it 'suggests nothing when there is no match' do
    session.run_command('echo foo')
    sleep 0.3
    session.clear_screen

    session.send_string('zzz')
    sleep 1
    expect(session.content).to eq('zzz')
  end

  it 'updates suggestion as user types more characters' do
    session.run_command('echo hello')
    session.run_command('echo hey')
    session.run_command('echo help')
    sleep 0.3
    session.clear_screen

    session.send_string('echo hel')
    wait_for { session.content }.to eq('echo help')
  end
end
