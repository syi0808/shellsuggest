# Adapted from zsh-users/zsh-autosuggestions (MIT License)
# Tests that shellsuggest preserves custom widget wrappers

describe 'a wrapped widget' do
  let(:widget) { 'forward-char' }

  context 'initialized before sourcing the plugin' do
    let(:before_sourcing) do
      -> do
        session.
          run_command("#{widget}-magic() { BUFFER+=X }").
          run_command("zle -N #{widget} #{widget}-magic")
      end
    end

    it 'accepts the suggestion and executes the custom behavior' do
      with_history('foobar') do
        session.send_string('foo')
        wait_for { session.content }.to eq('foobar')

        session.send_keys('Right')
        wait_for { session.content }.to eq('foobarX')
      end
    end
  end

  context 'initialized after sourcing the plugin' do
    before do
      session.
        run_command("#{widget}-magic() { BUFFER+=X }").
        run_command("zle -N #{widget} #{widget}-magic").
        clear_screen
    end

    it 're-binds on the next prompt and keeps suggestion acceptance working' do
      with_history('foobar') do
        session.send_string('foo')
        wait_for { session.content }.to eq('foobar')

        session.send_keys('Right')
        wait_for { session.content }.to eq('foobarX')
      end
    end
  end
end
