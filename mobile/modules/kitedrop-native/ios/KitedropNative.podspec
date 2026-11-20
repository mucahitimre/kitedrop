Pod::Spec.new do |s|
  s.name           = 'KitedropNative'
  s.version        = '0.1.0'
  s.summary        = 'Expo native module exposing the kitedrop core to the React Native UI'
  s.author         = 'kitedrop'
  s.homepage       = 'https://github.com/mucahitimre/kitedrop'
  s.license        = { :type => 'MIT' }
  s.platforms      = { :ios => '16.4' }
  s.source         = { git: '' }
  s.static_framework = true

  s.dependency 'ExpoModulesCore'
  # Rust core + shared Swift; its path is declared in targets/share/pods.rb (external source).
  s.dependency 'KitedropCore'

  s.pod_target_xcconfig = {
    'DEFINES_MODULE' => 'YES',
  }

  s.source_files = "**/*.{h,m,mm,swift,hpp,cpp}"
end
