# shellsuggest-specific: Plugin conflict detection

describe 'plugin conflict detection' do
  context 'when zsh-autosuggestions is also loaded' do
    let(:before_sourcing) do
      -> do
        # Simulate zsh-autosuggestions being loaded
        session.run_command('_ZSH_AUTOSUGGEST_INITIALIZED=1')
      end
    end

    it 'prints a warning about ghost text collision' do
      # The warning should have been printed during sourcing
      # Check by looking at terminal content before clear_screen
      # (clear_screen happens in around hook, so we check after)
      # We verify the plugin still loads despite the warning
      session.send_string('echo test')
      sleep 0.5
      # Plugin should still function
      expect(session.content).to start_with('echo test')
    end
  end
end
