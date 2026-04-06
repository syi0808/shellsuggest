# Tests feedback recording and status metrics

describe 'feedback metrics' do
  it 'records accepted suggestions in status output' do
    with_history('echo hello world') do
      session.send_string('echo h')
      wait_for { session.content }.to eq('echo hello world')

      session.send_keys('Right')
      wait_for { session.content }.to eq('echo hello world')
    end

    session.clear_screen
    session.run_command('shellsuggest status')
    wait_for { session.content }.to include('feedback.accepted: 1')
    expect(session.content).to include('feedback.acceptance_rate: 100.0%')
    expect(session.content).to include('config:')
  end

  it 'records rejected suggestions when clearing ghost text' do
    with_history('echo hello world') do
      session.send_string('echo h')
      wait_for { session.content }.to eq('echo hello world')

      session.send_keys('escape')
      wait_for { session.content }.to eq('echo h')
    end

    session.clear_screen
    session.run_command('shellsuggest status')
    wait_for { session.content }.to include('feedback.rejected: 1')
  end
end
