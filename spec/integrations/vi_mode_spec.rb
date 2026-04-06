# Ported from zsh-users/zsh-autosuggestions (MIT License)
# Tests vi mode compatibility

describe 'when using vi mode' do
  let(:before_sourcing) do
    -> do
      session.run_command('bindkey -v')
    end
  end

  describe 'moving the cursor after exiting insert mode' do
    it 'should not clear the current suggestion' do
      with_history('foobar foo') do
        session.
          send_string('foo').
          send_keys('escape').
          send_keys('h')

        wait_for { session.content }.to eq('foobar foo')
      end
    end
  end

  describe '`vi-forward-word-end`' do
    it 'accepts through the end of the current word' do
      with_history('foobar foo') do
        session.
          send_string('foo').
          send_keys('escape').
          send_keys('e').
          send_keys('a').
          send_string('baz')

        wait_for { session.content }.to eq('foobarbaz')
      end
    end
  end

  describe '`vi-forward-word`' do
    it 'accepts through the first character of the next word' do
      with_history('foobar foo') do
        session.
          send_string('foo').
          send_keys('escape').
          send_keys('w').
          send_keys('a').
          send_string('az')

        wait_for { session.content }.to eq('foobar faz')
      end
    end
  end
end
