# Compatibility checks for zsh-autosuggestions migrations

describe 'zsh-autosuggestions compatibility' do
  context 'with autosuggest widget bindings' do
    let(:after_sourcing) do
      lambda do
        session.run_command('bindkey "^x" autosuggest-accept')
      end
    end

    it 'keeps autosuggest-accept bindings working' do
      with_history('echo hello world') do
        session.send_string('echo h')
        wait_for { session.content }.to eq('echo hello world')

        session.send_keys('C-x')
        wait_for { session.content }.to eq('echo hello world')
      end
    end
  end

  context 'with migrated history ignore settings' do
    let(:before_sourcing) do
      lambda do
        session.run_command('export ZSH_AUTOSUGGEST_HISTORY_IGNORE="echo *"')
      end
    end

    it 'suppresses suggestions that match the old ignore pattern' do
      with_history('echo hello world') do
        session.send_string('echo')
        sleep 0.2
        expect(session.content).to eq('echo')
      end
    end
  end
end
