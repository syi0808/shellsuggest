# Ported from zsh-users/zsh-autosuggestions (MIT License)
# Tests paste handling when bracketed-paste-magic is active

describe 'pasting using bracketed-paste-magic' do
  let(:before_sourcing) do
    -> do
      session.
        run_command('autoload -Uz bracketed-paste-magic').
        run_command('zle -N bracketed-paste bracketed-paste-magic')
    end
  end

  it 'does not retain an old suggestion after paste' do
    with_history('echo foo') do
      session.send_string('echo ')
      wait_for { session.content }.to eq('echo foo')

      session.paste_string('bar')
      wait_for { session.content }.to eq('echo bar')

      session.send_keys('Right')
      sleep 0.2
      expect(session.content).to eq('echo bar')
    end
  end
end
