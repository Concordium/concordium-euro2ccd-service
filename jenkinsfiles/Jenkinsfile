pipeline {
    agent any
    environment {
        S3_BUCKET = "s3://euro2ccd.concordium.com"
        PKG_FILE = "concordium-eur2ccd_${version}_amd64.deb"
        OUT_FILE = "concordium-eur2ccd_${version}-${build}_amd64.deb"
        OUT_PATH = "${S3_BUCKET}/${OUT_FILE}"
        OUT_DIR = sh(script: 'mktemp -d', returnStdout: true).trim()
    }
    stages {
        
        stage('Build') {
            steps {
                sh '''\
                    docker build -t ccd-service-builder --build-arg ubuntu_version=${base_image} -f scripts/debian-package/deb.Dockerfile .

                    set -euxo pipefail

                    id=$(docker create ccd-service-builder)
                    docker cp $id:/build/pkg-root/ eur2ccd-deb
                    docker rm $id
                    mv eur2ccd-deb/$PKG_FILE .
                '''.stripIndent()
            }
        }
       stage('Publish') {
            steps {
                sh '''\
                    # Push to S3.
                    aws s3 cp ${PKG_FILE} "${OUT_PATH}" 
                '''.stripIndent()
            }
        }
    }
}
